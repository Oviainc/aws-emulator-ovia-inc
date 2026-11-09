Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.url-aws-iam-signature-scoped-to-another-service-rejected lambda.url-aws-iam-signed-request-429-when-reserved-concurrency-zero lambda.url-aws-iam-tampered-body-rejected-with-invalid-signature lambda.url-aws-iam-unsigned-request-forbidden-signed-request-served-with-caller-identity lambda.url-binary-request-body-reaches-handler-base64-encoded lambda.url-cookie-header-split-into-event-cookies-array lambda.url-deleted-config-stops-serving-the-url lambda.url-form-urlencoded-body-is-base64-encoded-while-text-plain-is-not lambda.url-function-timeout-rejected-with-502 lambda.url-get-event-v2-carries-raw-path-query-and-lowercased-headers lambda.url-invalid-status-code-result-rejected-with-502 lambda.url-none-auth-grants-to-a-named-principal-do-not-admit-anonymous-callers lambda.url-on-alias-qualifier-routes-to-the-alias-version-and-follows-alias-updates lambda.url-plain-string-result-served-as-bare-text-unlike-invoke lambda.url-response-base64-body-decoded-to-raw-bytes lambda.url-response-cookies-emitted-as-set-cookie-headers lambda.url-response-over-6mb-rejected-with-502 lambda.url-response-status-headers-and-body-mapped-from-handler-result lambda.url-result-without-content-type-or-body-gets-json-default-and-empty-body lambda.url-result-without-status-code-inferred-as-200-json lambda.url-text-request-body-reaches-handler-verbatim-not-base64 lambda.url-unhandled-function-error-rejected-with-502-without-leaking-the-error lambda.deep-lambda-3-url-auth-without-invoke-function-grant-denied lambda.deep-lambda-3-url-cors-disallowed-preflight-rejected lambda.deep-lambda-3-url-cors-not-enforced-server-side lambda.deep-lambda-3-url-cors-preflight-answered-without-invoking-handler lambda.function-url-missing-permission-not-invoked lambda.function-url-response-stream-mode-chunks-over-https lambda.stream-url-error-after-first-write-truncates-the-200-body lambda.stream-url-http-response-stream-metadata-sets-status-headers-and-cookies lambda.code-location-serves-the-exact-bytes-and-a-published-version-is-byte-frozen lambda.callback-heartbeat-extends-deadline lambda.callback-no-heartbeat-times-out lambda.heartbeat-extends-callback-window lambda.durable-execution-checkpoint-ttl-bound lambda.telemetry-api-subscribe-validation-rejected-with-exact-error-bodies lambda.restore-next-without-snapstart-returns-404 lambda.timeout-in-an-update-refused-for-another-field-is-not-applied-rejected lambda.nodejs-task-timed-out-discards-the-execution-environment-rejected lambda.timeout-lowered-kills-a-handler-that-finished-before-rejected lambda.nodejs-event-invocation-runs-the-handler-and-logs-its-digest-under-the-request-id lambda.nodejs-function-sdk-v3-writes-binary-bytes-the-caller-reads-back
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.url-aws-iam-signature-scoped-to-another-service-rejected|_create_url#4|AssertionError|assert not (p := ctx.published_url_mismatch(cfg["FunctionUrl"])), p
Issue-Key: lambda.url-aws-iam-signed-request-429-when-reserved-concurrency-zero|_create_url#4|AssertionError|assert not (p := ctx.published_url_mismatch(cfg["FunctionUrl"])), p
Issue-Key: lambda.url-aws-iam-tampered-body-rejected-with-invalid-signature|_create_url#4|AssertionError|assert not (p := ctx.published_url_mismatch(cfg["FunctionUrl"])), p
Issue-Key: lambda.url-aws-iam-unsigned-request-forbidden-signed-request-served-with-caller-identity|_create_url#4|AssertionError|assert not (p := ctx.published_url_mismatch(cfg["FunctionUrl"])), p
Issue-Key: lambda.url-binary-request-body-reaches-handler-base64-encoded|_create_url#4|AssertionError|assert not (p := ctx.published_url_mismatch(cfg["FunctionUrl"])), p
Issue-Key: lambda.url-cookie-header-split-into-event-cookies-array|_create_url#4|AssertionError|assert not (p := ctx.published_url_mismatch(cfg["FunctionUrl"])), p
Issue-Key: lambda.url-deleted-config-stops-serving-the-url|_create_url#4|AssertionError|assert not (p := ctx.published_url_mismatch(cfg["FunctionUrl"])), p
Issue-Key: lambda.url-form-urlencoded-body-is-base64-encoded-while-text-plain-is-not|_create_url#4|AssertionError|assert not (p := ctx.published_url_mismatch(cfg["FunctionUrl"])), p
Issue-Key: lambda.url-function-timeout-rejected-with-502|_create_url#4|AssertionError|assert not (p := ctx.published_url_mismatch(cfg["FunctionUrl"])), p
Issue-Key: lambda.url-get-event-v2-carries-raw-path-query-and-lowercased-headers|_create_url#4|AssertionError|assert not (p := ctx.published_url_mismatch(cfg["FunctionUrl"])), p
Issue-Key: lambda.url-invalid-status-code-result-rejected-with-502|_create_url#4|AssertionError|assert not (p := ctx.published_url_mismatch(cfg["FunctionUrl"])), p
Issue-Key: lambda.url-none-auth-grants-to-a-named-principal-do-not-admit-anonymous-callers|_create_url#4|AssertionError|assert not (p := ctx.published_url_mismatch(cfg["FunctionUrl"])), p
Issue-Key: lambda.url-on-alias-qualifier-routes-to-the-alias-version-and-follows-alias-updates|_create_url#10|AssertionError|assert not (p := ctx.published_url_mismatch(cfg["FunctionUrl"])), p
Issue-Key: lambda.url-plain-string-result-served-as-bare-text-unlike-invoke|_create_url#4|AssertionError|assert not (p := ctx.published_url_mismatch(cfg["FunctionUrl"])), p
Issue-Key: lambda.url-response-base64-body-decoded-to-raw-bytes|_create_url#4|AssertionError|assert not (p := ctx.published_url_mismatch(cfg["FunctionUrl"])), p
Issue-Key: lambda.url-response-cookies-emitted-as-set-cookie-headers|_create_url#4|AssertionError|assert not (p := ctx.published_url_mismatch(cfg["FunctionUrl"])), p
Issue-Key: lambda.url-response-over-6mb-rejected-with-502|_create_url#4|AssertionError|assert not (p := ctx.published_url_mismatch(cfg["FunctionUrl"])), p
Issue-Key: lambda.url-response-status-headers-and-body-mapped-from-handler-result|_create_url#4|AssertionError|assert not (p := ctx.published_url_mismatch(cfg["FunctionUrl"])), p
Issue-Key: lambda.url-result-without-content-type-or-body-gets-json-default-and-empty-body|_create_url#4|AssertionError|assert not (p := ctx.published_url_mismatch(cfg["FunctionUrl"])), p
Issue-Key: lambda.url-result-without-status-code-inferred-as-200-json|_create_url#4|AssertionError|assert not (p := ctx.published_url_mismatch(cfg["FunctionUrl"])), p
Issue-Key: lambda.url-text-request-body-reaches-handler-verbatim-not-base64|_create_url#4|AssertionError|assert not (p := ctx.published_url_mismatch(cfg["FunctionUrl"])), p
Issue-Key: lambda.url-unhandled-function-error-rejected-with-502-without-leaking-the-error|_create_url#4|AssertionError|assert not (p := ctx.published_url_mismatch(cfg["FunctionUrl"])), p
Issue-Key: lambda.deep-lambda-3-url-auth-without-invoke-function-grant-denied|_open_url#10|AssertionError|assert not (p := ctx.published_url_mismatch(url)), p
Issue-Key: lambda.deep-lambda-3-url-cors-disallowed-preflight-rejected|_open_url#8|AssertionError|assert not (p := ctx.published_url_mismatch(url)), p
Issue-Key: lambda.deep-lambda-3-url-cors-not-enforced-server-side|_open_url#8|AssertionError|assert not (p := ctx.published_url_mismatch(url)), p
Issue-Key: lambda.deep-lambda-3-url-cors-preflight-answered-without-invoking-handler|_open_url#8|AssertionError|assert not (p := ctx.published_url_mismatch(url)), p
Issue-Key: lambda.function-url-missing-permission-not-invoked|run#12|AssertionError|assert not (p := ctx.published_url_mismatch(url)), p
Issue-Key: lambda.function-url-response-stream-mode-chunks-over-https|run#11|AssertionError|assert not (p := ctx.published_url_mismatch(url)), p
Issue-Key: lambda.stream-url-error-after-first-write-truncates-the-200-body|_create_url#4|AssertionError|assert not (p := ctx.published_url_mismatch(cfg["FunctionUrl"])), p
Issue-Key: lambda.stream-url-http-response-stream-metadata-sets-status-headers-and-cookies|_create_url#4|AssertionError|assert not (p := ctx.published_url_mismatch(cfg["FunctionUrl"])), p
Issue-Key: lambda.code-location-serves-the-exact-bytes-and-a-published-version-is-byte-frozen|_download#16|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: lambda.callback-heartbeat-extends-deadline|run#9|AssertionError|raise _never_ready(
Issue-Key: lambda.callback-no-heartbeat-times-out|run#6|AssertionError|raise AssertionError(
Issue-Key: lambda.heartbeat-extends-callback-window|_poll_until#9|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: lambda.durable-execution-checkpoint-ttl-bound|run#12|AssertionError|assert 15 <= elapsed < 100, (
Issue-Key: lambda.telemetry-api-subscribe-validation-rejected-with-exact-error-bodies|run#5|AssertionError|assert resp["StatusCode"] == 200 and "FunctionError" not in resp, (
Issue-Key: lambda.restore-next-without-snapstart-returns-404|_probe#8|AssertionError|assert resp["StatusCode"] == 200 and "FunctionError" not in resp, (
Issue-Key: lambda.timeout-in-an-update-refused-for-another-field-is-not-applied-rejected|_assert_timed_out#6|AssertionError|assert body.get("errorMessage") == want, (
Issue-Key: lambda.nodejs-task-timed-out-discards-the-execution-environment-rejected|run#11|AssertionError|assert body.get("errorMessage") == want, (
Issue-Key: lambda.timeout-lowered-kills-a-handler-that-finished-before-rejected|run#8|AssertionError|assert isinstance(entry, int) and (_LONG_S - 1) * 1000 < entry <= _LONG_S * 1000, (
Issue-Key: lambda.nodejs-event-invocation-runs-the-handler-and-logs-its-digest-under-the-request-id|_digest_lines#12|AssertionError|assert len(fields) == 4, f"console.log line is not timestamp/request id/level/text: {e['message']!r}"
Issue-Key: lambda.nodejs-function-sdk-v3-writes-binary-bytes-the-caller-reads-back|_invoke#20|AssertionError|raise AssertionError(
Issue-Commit: bc4720188099afd2bb4f421d1e83533814225f44

# 10 net-new causes: Lambda publishes unreachable data-plane endpoints and its runtime-API/durable-execution surfaces are inert

> _Note: this issue was identified against the code base at commit `bc4720188099afd2bb4f421d1e83533814225f44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunctionUrlConfig

## Description
Ten distinct root causes in Lambda's data plane. They are separate work items; all ten must be closed. (An open issue, 20261109T152222Z-apigateway-lambda-function-urls-are-published-on-lambda.md, carries the same first cause for the apigateway__lambda checks; it does not cover the lambda-service checks listed here.)

Cause 1 of 10 — the function URL is published on `<id>.lambda-url.<region>.on.aws`, a host the emulator does not serve.
AWS: Smithy CreateFunctionUrlConfig/GetFunctionUrlConfig return a required FunctionUrl (length 40..100). In AWS that URL resolves to the service that minted it; the behavioral requirement is that the announced address is the address the service answers on, with AWS's scheme and trailing-slash path shape preserved.
Emulator: publishes `https://<32-hex>.lambda-url.<region>.on.aws/`, which does not resolve to the emulator; the emulator is reachable only on its own host and port. Every request against the published URL therefore never reaches Lambda, and 30 function-URL checks (auth modes, CORS, event shaping, response mapping, streaming) fail before exercising any URL behavior.
Why: the function URL is the only address a client has for a URL-invoked function; announcing an unreachable third-party host makes the entire function-URL data plane unusable and turns every call into a DNS/egress failure rather than a Lambda response.
Fix: the function-URL endpoint serializer/state that mints FunctionUrl — publish the emulator's own host and port (or a per-resource listener it owns).
Traces: lambda.deep-lambda-3-url-auth-without-invoke-function-grant-denied, lambda.deep-lambda-3-url-cors-disallowed-preflight-rejected, lambda.deep-lambda-3-url-cors-not-enforced-server-side, lambda.deep-lambda-3-url-cors-preflight-answered-without-invoking-handler, lambda.function-url-missing-permission-not-invoked, lambda.function-url-response-stream-mode-chunks-over-https, lambda.stream-url-error-after-first-write-truncates-the-200-body, lambda.stream-url-http-response-stream-metadata-sets-status-headers-and-cookies, and the 22 lambda.url-* checks listed in trace_ids.

Cause 2 of 10 — GetFunction's Code.Location is published on awslambda-<region>-tasks.s3.<region>.amazonaws.com.
AWS: Smithy FunctionCodeLocation.Location is documented as "A presigned URL that you can use to download the deployment package" — a URL the caller can actually GET to retrieve the exact bytes uploaded, and a published version's location serves bytes frozen at publish time.
Emulator: publishes `https://awslambda-<region>-tasks.s3.<region>.amazonaws.com/snapshots/<account>/<fn>-<uuid>`, a host it does not serve, so the package can never be downloaded.
Why: a client that reads back the deployment package (code-integrity checks, promotion pipelines) cannot fetch it at all.
Fix: the GetFunction code-location serializer — presign against the emulator's own host/port for the object store it actually serves.
Traces: lambda.code-location-serves-the-exact-bytes-and-a-published-version-is-byte-frozen.

Cause 3 of 10 — the durable-execution callback plane never fires.
AWS: a durable function that calls wait_for_callback reaches a callback checkpoint, which appears in the execution history as a CallbackStartedDetails event and delivers a CallbackId through the configured notify callback; a heartbeat extends the callback deadline and SendCallbackSuccess resolves it, after which the execution's destination receives the record.
Emulator: the checkpoint is never reached — no CallbackStartedDetails event appears, no CallbackId is ever delivered, and the destination queue stays empty for the whole poll window.
Why: every callback-based durable workflow hangs; a client waiting for a CallbackId waits forever with nothing in the history to explain it.
Fix: the durable-execution runtime's callback checkpoint handling (history writer, CallbackId minting, notify delivery and heartbeat deadline accounting).
Traces: lambda.callback-heartbeat-extends-deadline, lambda.callback-no-heartbeat-times-out, lambda.heartbeat-extends-callback-window.

Cause 4 of 10 — a durable execution with a 30 s ExecutionTimeout fails after ~9 s.
AWS: a durable execution runs until its work completes or its own ExecutionTimeout elapses; a 30 s timeout forces the execution out at ~30 s, not materially sooner, and the terminal status reflects that timeout.
Emulator: the execution reaches FAILED in ~9 s — well inside its own 30 s ExecutionTimeout and far short of the in-flight work it was given.
Why: a client cannot rely on ExecutionTimeout at all: work that AWS would let run is cut short, and the failure is attributed to the execution rather than to a timeout the client configured.
Fix: the durable-execution state machine's timeout/TTL accounting for checkpoints (the bound must come from the execution's configured ExecutionTimeout).
Traces: lambda.durable-execution-checkpoint-ttl-bound.

Cause 5 of 10 — the Extensions/Telemetry API register endpoint is not served.
AWS: an extension or a handler reaching `http://$AWS_LAMBDA_RUNTIME_API/2020-01-01/extension/register` gets the Extensions API; Telemetry API subscribe then validates its body and answers with documented error bodies.
Emulator: the in-sandbox HTTP client reports that `http://127.0.0.1:9001/2020-01-01/extension/register` "addresses no service this emulator serves", and lists only AWS service names — the runtime API's extension paths are not routed at all, so the handler fails to import.
Why: no extension can register, and nothing of the Telemetry API's validation contract is observable.
Fix: the in-sandbox runtime-API router — serve the /2020-01-01/extension/* and telemetry paths on the runtime API endpoint.
Traces: lambda.telemetry-api-subscribe-validation-rejected-with-exact-error-bodies.

Cause 6 of 10 — the SnapStart restore/next runtime-API endpoint is not served.
AWS: `GET http://$AWS_LAMBDA_RUNTIME_API/2018-06-01/runtime/restore/next` on a function without SnapStart answers HTTP 404; the probing invoke itself succeeds and reports that status.
Emulator: the request fails inside the sandbox with a gaierror — `http://127.0.0.1:9001/2018-06-01/runtime/restore/next addresses no service this emulator serves` — so the invoke returns FunctionError='Unhandled' instead of the handler reporting a 404.
Why: a runtime or extension probing the restore endpoint (which every SnapStart-aware runtime does) crashes instead of reading a 404.
Fix: the in-sandbox runtime-API router — route /2018-06-01/runtime/restore/next and answer 404 when the function has no SnapStart snapshot.
Traces: lambda.restore-next-without-snapstart-returns-404.

Cause 7 of 10 — a task-timeout errorMessage omits AWS's `RequestId: <id> Error: ` prefix.
AWS: when an invocation exceeds its Timeout, the error payload's errorMessage is `RequestId: <request id> Error: Task timed out after <seconds>.<2dp> seconds`. The fixed parts are `RequestId: `, ` Error: Task timed out after ` and ` seconds`; the variable parts are the invocation's request id and the function's configured Timeout formatted to two decimals.
Emulator: emits `<request id> Task timed out after <seconds>.00 seconds` — the request id with no `RequestId: ` label and no ` Error: ` separator.
Why: a client that parses the timeout message (log scrapers, error classifiers, the Node/Python runtime's own error shaping) cannot match AWS's line, so timeouts are reported as unclassified errors.
Fix: the invoke timeout error serializer in the runtime/invoke path.
Traces: lambda.timeout-in-an-update-refused-for-another-field-is-not-applied-rejected, lambda.nodejs-task-timed-out-discards-the-execution-environment-rejected.

Cause 8 of 10 — getRemainingTimeInMillis at handler entry is below the configured Timeout.
AWS: at handler entry the remaining time is the function's configured Timeout converted to milliseconds, less only the few milliseconds already spent in the invoke; a 7 s Timeout therefore reports close to 7000 ms, and init is not charged against it.
Emulator: reports 5940 ms at entry under Timeout=7 — more than a second of the budget is already gone before the handler starts.
Why: handlers that reserve part of the remaining budget (to flush, checkpoint or return a partial result before the deadline) mis-plan, and a handler that AWS would let finish is killed.
Fix: the invoke deadline computation handed to the runtime (the deadline must be derived from the configured Timeout at the moment the handler is entered).
Traces: lambda.timeout-lowered-kills-a-handler-that-finished-before-rejected.

Cause 9 of 10 — Node console output is not framed as a log line.
AWS: in Text log format a Node handler's `console.log` reaches CloudWatch as `<ISO-8601 timestamp>\t<request id>\t<level>\t<text>` — four tab-separated fields; the timestamp, request id and text are per-invocation values, the field order and the tab framing are the requirement.
Emulator: delivers the bare text (`DIGEST <...>`) with no timestamp, request id or level.
Why: a client parsing its function's logs cannot correlate a line to an invocation or filter by level, which is the whole purpose of the framing.
Fix: the Node runtime's console/stdout log framer in the invoke log pipeline.
Traces: lambda.nodejs-event-invocation-runs-the-handler-and-logs-its-digest-under-the-request-id.

Cause 10 of 10 — the Node runtime's bundled @aws-sdk/client-sts exports no GetCallerIdentityCommand.
AWS: the managed Node runtimes bundle the AWS SDK v3; `import { GetCallerIdentityCommand } from '@aws-sdk/client-sts'` resolves and the command runs against STS.
Emulator: the handler fails at load with `Runtime.UserCodeSyntaxError` — `SyntaxError: Could not find export 'GetCallerIdentityCommand' in module 'file:///var/runtime/node_modules/@aws-sdk/client-sts'`.
Why: any Node handler that uses the bundled SDK v3 against STS cannot even be imported, so it fails as a user syntax error rather than running.
Fix: the Node runtime's bundled SDK v3 client surface (@aws-sdk/client-sts must export its commands).
Traces: lambda.nodejs-function-sdk-v3-writes-binary-bytes-the-caller-reads-back.

## Evidence
Cause 1 (30 checks, identical shape): AssertionError: the emulator published https://<32-hex>.lambda-url.<region>.on.aws/: its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://<own host>:<port>/<resource path>.
Cause 2: simparity.published.PublishedUrlRejected: endpoint_url was given https://awslambda-<region>-tasks.s3.<region>.amazonaws.com/snapshots/<account>/<fn>-<uuid> ... its host is not one of the emulator's own hosts.
Cause 3: AssertionError: durable execution <arn>/durable-execution/<name>/<id> never delivered a CallbackId over <queue url> (the notify callback passed to wait_for_callback never fired within the poll window); ... never reached the wait_for_callback checkpoint (no CallbackStartedDetails history event appeared); a message to arrive on <queue url> not met within 60s. Last result: None.
Cause 4: AssertionError: execution reached FAILED in 9.0s; its own 30s ExecutionTimeout should force it out well before the 120s in-flight wait would complete naturally, and not implausibly fast.
Cause 5: AssertionError: the probing invoke must itself succeed: FunctionError='Unhandled', log "[ERROR] Runtime.ImportModuleError: ... http://127.0.0.1:9001/2020-01-01/extension/register addresses no service this emulator serves: sqs, events, ... rds-data".
Cause 6: AssertionError: the probing invoke ... must itself succeed: StatusCode=200, FunctionError='Unhandled', log "[ERROR] gaierror: http://127.0.0.1:9001/2018-06-01/runtime/restore/next addresses no service this emulator serves: ...".
Cause 7: AssertionError: errorMessage '<request id> Task timed out after 2.00 seconds', expected 'RequestId: <request id> Error: Task timed out after 2.00 seconds' (same for a 5.00 s timeout in the nodejs check).
Cause 8: AssertionError: remaining time at entry under Timeout=7 was 5940 ms.
Cause 9: AssertionError: console.log line is not timestamp/request id/level/text: 'DIGEST <...>'.
Cause 10: AssertionError: the function raised (Unhandled): {"errorMessage":"SyntaxError: Could not find export 'GetCallerIdentityCommand' in module 'file:///var/runtime/node_modules/@aws-sdk/client-sts'","errorType":"Runtime.UserCodeSyntaxError"}.
(hosts, ports, uuids, request ids, queue URLs and timestamps in these lines are per-run values, never requirements.)

## Steps to Reproduce
Cause 1: CreateFunction, then CreateFunctionUrlConfig(AuthType='NONE'); GET the returned FunctionUrl — it does not reach the emulator.
Cause 2: CreateFunction, then GetFunction and GET Code.Location.
Cause 3: Create a durable function that calls wait_for_callback with a notify destination; Invoke it and poll the execution history for CallbackStartedDetails and the destination queue for the CallbackId.
Cause 4: Start a durable execution with ExecutionTimeout=30 whose work would run longer; observe it ends FAILED after ~9 s.
Cause 5: From a handler (or extension) POST to $AWS_LAMBDA_RUNTIME_API/2020-01-01/extension/register.
Cause 6: From a handler on a function without SnapStart, GET $AWS_LAMBDA_RUNTIME_API/2018-06-01/runtime/restore/next and report the status.
Cause 7: Invoke a handler that sleeps past its Timeout and read the error payload's errorMessage.
Cause 8: Create a function with Timeout=7 and return context.get_remaining_time_in_millis() at handler entry.
Cause 9: Invoke a nodejs function that calls console.log and read the delivered log line's framing.
Cause 10: Invoke a nodejs function that imports GetCallerIdentityCommand from @aws-sdk/client-sts.
