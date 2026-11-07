Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.url-aws-iam-signature-scoped-to-another-service-rejected lambda.url-aws-iam-tampered-body-rejected-with-invalid-signature lambda.url-aws-iam-unsigned-request-forbidden-signed-request-served-with-caller-identity lambda.url-binary-request-body-reaches-handler-base64-encoded lambda.url-cookie-header-split-into-event-cookies-array lambda.url-deleted-config-stops-serving-the-url lambda.url-form-urlencoded-body-is-base64-encoded-while-text-plain-is-not lambda.url-function-timeout-rejected-with-502 lambda.url-get-event-v2-carries-raw-path-query-and-lowercased-headers lambda.url-invalid-status-code-result-rejected-with-502 lambda.url-none-auth-grants-to-a-named-principal-do-not-admit-anonymous-callers lambda.url-on-alias-qualifier-routes-to-the-alias-version-and-follows-alias-updates lambda.url-plain-string-result-served-as-bare-text-unlike-invoke lambda.url-response-base64-body-decoded-to-raw-bytes lambda.url-response-cookies-emitted-as-set-cookie-headers lambda.url-response-over-6mb-rejected-with-502 lambda.url-response-status-headers-and-body-mapped-from-handler-result lambda.url-result-without-content-type-or-body-gets-json-default-and-empty-body lambda.url-result-without-status-code-inferred-as-200-json lambda.url-text-request-body-reaches-handler-verbatim-not-base64 lambda.url-unhandled-function-error-rejected-with-502-without-leaking-the-error lambda.stream-url-error-after-first-write-truncates-the-200-body lambda.stream-url-http-response-stream-metadata-sets-status-headers-and-cookies lambda.durable-execution-callback-already-resolved-rejected lambda.durable-execution-callback-failure-flow lambda.durable-execution-callback-success-flow lambda.durable-execution-history-ordering lambda.durable-execution-lifecycle lambda.list-durable-executions-by-function lambda.stop-durable-execution-terminal-state-is-a-no-op lambda.send-callback-kms-errors lambda.cli-lambda-create-invoke lambda.cli-lambda-invoke-nonexistent-error lambda.alias-weight-ladder-observed-distribution lambda.container-image-update-code lambda.update-function-code-dry-run-changes-nothing-at-runtime lambda.log-tail-python-json-levels-keep-python-level-names
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.url-aws-iam-signature-scoped-to-another-service-rejected|_poll_until#5|Timeout|time.sleep(interval_s)
Issue-Key: lambda.url-aws-iam-tampered-body-rejected-with-invalid-signature|_poll_until#6|Timeout|time.sleep(interval_s)
Issue-Key: lambda.url-aws-iam-unsigned-request-forbidden-signed-request-served-with-caller-identity|run#7|AssertionError|assert unsigned["status"] == 403, f"an unsigned request must be refused 403, got {unsigned!r}"
Issue-Key: lambda.url-binary-request-body-reaches-handler-base64-encoded|run#7|AssertionError|assert resp["status"] == 200, f"{content_type}: the POST must be served, got {resp!r}"
Issue-Key: lambda.url-cookie-header-split-into-event-cookies-array|run#7|AssertionError|assert with_cookies["status"] == 200, f"the GET must be served, got {with_cookies!r}"
Issue-Key: lambda.url-deleted-config-stops-serving-the-url|run#7|AssertionError|assert served["status"] == 200, f"positive control: the URL must serve first, got {served!r}"
Issue-Key: lambda.url-form-urlencoded-body-is-base64-encoded-while-text-plain-is-not|run#8|AssertionError|assert form["status"] == 200, f"the form POST must be served, got {form!r}"
Issue-Key: lambda.url-function-timeout-rejected-with-502|run#7|AssertionError|assert (quick["status"], quick["body"]) == (200, b"quick"), f"positive control failed: {quick!r}"
Issue-Key: lambda.url-get-event-v2-carries-raw-path-query-and-lowercased-headers|run#8|AssertionError|assert resp["status"] == 200, f"the echo GET must be served 200, got {resp!r}"
Issue-Key: lambda.url-invalid-status-code-result-rejected-with-502|run#7|AssertionError|assert (valid["status"], valid["body"]) == (200, b"valid"), (
Issue-Key: lambda.url-none-auth-grants-to-a-named-principal-do-not-admit-anonymous-callers|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Key: lambda.url-on-alias-qualifier-routes-to-the-alias-version-and-follows-alias-updates|run#17|AssertionError|assert json.loads(alias_resp["body"]) == {"code": "one", "version": "1"}, (
Issue-Key: lambda.url-plain-string-result-served-as-bare-text-unlike-invoke|run#7|AssertionError|assert resp["status"] == 200, f"a string result is a 200, got {resp!r}"
Issue-Key: lambda.url-response-base64-body-decoded-to-raw-bytes|run#8|AssertionError|assert binary["status"] == 200, f"the GET must be served, got {binary!r}"
Issue-Key: lambda.url-response-cookies-emitted-as-set-cookie-headers|run#7|AssertionError|assert resp["status"] == 200, f"the GET must be served, got {resp!r}"
Issue-Key: lambda.url-response-over-6mb-rejected-with-502|run#7|AssertionError|assert small["status"] == 200, f"a 1 MiB body must be served, got status {small['status']!r}"
Issue-Key: lambda.url-response-status-headers-and-body-mapped-from-handler-result|run#7|AssertionError|assert created["status"] == 201, f"statusCode 201 must become the status, got {created!r}"
Issue-Key: lambda.url-result-without-content-type-or-body-gets-json-default-and-empty-body|run#7|AssertionError|assert accepted["status"] == 202, f"statusCode 202 must be the status, got {accepted!r}"
Issue-Key: lambda.url-result-without-status-code-inferred-as-200-json|run#7|AssertionError|assert resp["status"] == 200, f"{mode}: an inferred response is 200, got {resp!r}"
Issue-Key: lambda.url-text-request-body-reaches-handler-verbatim-not-base64|run#7|AssertionError|assert resp["status"] == 200, f"{content_type}: the POST must be served, got {resp!r}"
Issue-Key: lambda.url-unhandled-function-error-rejected-with-502-without-leaking-the-error|run#7|AssertionError|assert (ok["status"], ok["body"]) == (200, b"fine"), f"positive control failed: {ok!r}"
Issue-Key: lambda.stream-url-error-after-first-write-truncates-the-200-body|run#7|AssertionError|assert (full["status"], full["body"]) == (200, b"part-0;part-1;part-2;done"), (
Issue-Key: lambda.stream-url-http-response-stream-metadata-sets-status-headers-and-cookies|run#8|AssertionError|assert shaped["status"] == 201, f"the prelude's statusCode must be the status, got {shaped!r}"
Issue-Key: lambda.durable-execution-callback-already-resolved-rejected|_wait_for_callback_id#5|Timeout|time.sleep(2)
Issue-Key: lambda.durable-execution-callback-failure-flow|_receive_callback_id#7|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=1, WaitTimeSeconds=10)
Issue-Key: lambda.durable-execution-callback-success-flow|_wait_for_callback_id#6|Timeout|time.sleep(3.0)
Issue-Key: lambda.durable-execution-history-ordering|_wait_for_execution_status#4|Timeout|time.sleep(3)
Issue-Key: lambda.durable-execution-lifecycle|_poll_until#10|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Key: lambda.list-durable-executions-by-function|_poll_until#11|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Key: lambda.stop-durable-execution-terminal-state-is-a-no-op|_poll_until#6|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Key: lambda.send-callback-kms-errors|_start_and_get_callback#13|Timeout|msgs = sqs.receive_message(
Issue-Key: lambda.cli-lambda-create-invoke|_run_shell#2|AssertionError|raise AssertionError(
Issue-Key: lambda.cli-lambda-invoke-nonexistent-error|_run_shell#2|AssertionError|raise AssertionError(
Issue-Key: lambda.alias-weight-ladder-observed-distribution|_poll#16|Timeout|time.sleep(interval_s)
Issue-Key: lambda.container-image-update-code|run#12|AssertionError|assert resp.get("CodeSha256")
Issue-Key: lambda.update-function-code-dry-run-changes-nothing-at-runtime|run#9|AssertionError|assert dry["CodeSha256"] == _sha(new_zip), (
Issue-Key: lambda.log-tail-python-json-levels-keep-python-level-names|run#6|AssertionError|assert (lc.get("ApplicationLogLevel"), lc.get("SystemLogLevel")) == ("INFO", "INFO"), (
Issue-Commit: 221dd3602e5d0e66d71e70da1fb8509b07c948c5

# Lambda data plane: function URL endpoints are unrouted, durable executions never progress, and invoke-dependent checks hang (6 causes)

> _Note: this issue was identified against the code base at commit `221dd3602e5d0e66d71e70da1fb8509b07c948c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Six net-new root causes in the Lambda data plane, all grounded in failing behavioral checks of the `lambda` suite group. None of them is marked ALREADY FILED or OWNED in the replay report.

Cause 1 of 6 - The function URL endpoint is not routed, so no Function URL request is ever served.
AWS: a function URL config created by CreateFunctionUrlConfig is served at its own https endpoint; a GET to it returns the handler's mapped response (status 200 with the handler body for a well-formed result), and AuthType=AWS_IAM requests are evaluated for SigV4 before any invocation (403/Forbidden for an unsigned or wrongly-scoped signature). Smithy: Lambda's CreateFunctionUrlConfig returns FunctionUrl, and the URL host is a data-plane endpoint of the service, not a control-plane API; the payload/response contract is the documented payload format v2.0 (rawPath, rawQueryString, lower-cased headers, cookies array, isBase64Encoded, statusCode/headers/cookies/body result mapping). Recorded behavior for the URL host itself: a real request is answered by the function.
Emulator: every request to the function URL host is answered `404` with `content-type: application/x-amz-json-1.1`, `x-amzn-errortype: UnknownService` and body `{"__type":"UnknownService","message":"no emulated service is registered for 'lambda'"}` - i.e. the URL hostname is dispatched through the generic service router, which has no route for a function URL at all. Fixed text of that message is `no emulated service is registered for '<token>'`, with `<token>` taken from the request's host; it is not an AWS error shape (AWS never answers UnknownService on a function URL). Sibling checks that sign the request with SigV4 instead hang until the 60s check deadline (`Check timed out after 60.0s`) because the same unrouted host never completes the exchange.
Why: a client that deploys a function URL and calls it gets a 404 UnknownService JSON blob instead of its handler's response, so every URL-fronted application (REST handler, webhook receiver, streaming response reader) is non-functional, and negative tests see a 404 instead of the 403/502 AWS would return.
Fix: the function URL data-plane listener/router for the Lambda service (host-based routing to a function URL config, payload-format-v2.0 request mapping, result mapping, AWS_IAM SigV4 authorization, and the 502 classes for malformed/oversize/erroring handler results). Related open issue `20261023T114649Z-lambda-function-urls-unrouted-configuration-blocks-dropped-from.md` is about function-url CONFIGURATION blocks; it does not carry any of the replay keys below and does not cover the data-plane routing failure.
Traces: lambda.url-get-event-v2-carries-raw-path-query-and-lowercased-headers, lambda.stream-url-error-after-first-write-truncates-the-200-body (both show the 404 UnknownService body verbatim); the remaining url-*/stream-url-* ids listed in trace_ids.

Cause 2 of 6 - Durable executions are accepted but never run: Status stays RUNNING forever and callbacks never resolve.
AWS: Invoke with X-Amz-Durable-Execution-Name starts a durable execution that progresses to a terminal Status (SUCCEEDED/FAILED/STOPPED) and whose history is append-only and ordered; SendDurableExecutionCallback resolves a pending callback once and a second resolution of the same callback is refused; StopDurableExecution on an already-terminal execution is a no-op. Smithy: Invoke declares DurableExecutionArn output and DurableExecutionAlreadyStartedException (409); GetDurableExecutionState/ListDurableExecutions report Status.
Emulator: the execution is created (200 with DurableExecutionArn, Status=RUNNING, DurableConfig echoed) but never leaves `RUNNING` - polls fail with `condition not met within 45.0s; last={... 'Status': 'RUNNING' ...}` - and the callback checks hang to `Check timed out after 60.0s`. ListDurableExecutions returns the executions, all stuck RUNNING.
Why: a client that starts a durable execution and waits for it (the normal pattern) waits forever; callback-driven workflows never resume and never report failure.
Fix: the durable-execution runtime/state machine behind Invoke + SendDurableExecutionCallback + StopDurableExecution (execution progression to a terminal state, callback resolution with its already-resolved refusal, history ordering).
Traces: lambda.durable-execution-lifecycle, lambda.list-durable-executions-by-function, lambda.stop-durable-execution-terminal-state-is-a-no-op, lambda.durable-execution-callback-success-flow, lambda.durable-execution-callback-failure-flow, lambda.durable-execution-callback-already-resolved-rejected, lambda.durable-execution-history-ordering, lambda.send-callback-kms-errors.

Cause 3 of 6 - CLI and weight-distribution checks are refused or hang on Invoke.
AWS: `aws lambda invoke --function-name <fn>` runs the deployment package and returns StatusCode 200 with the handler payload; invoking a weighted alias repeatedly distributes executions across the alias' versions.
Emulator: the call is refused with HTTP error code `NotImplemented` and the message `Invoke resolved <function-arn> at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator: there is no in-process runtime for it, and no response payload is returned that the function did not produce. InvocationType=DryRun validates an invocation without running the function.` (fixed text with the resolved ARN, qualifier and runtime taken from the request/state); the aws CLI exits rc=254. The weighted-alias distribution check instead hangs to `Check timed out after 60.0s`.
Why: no CLI-driven or multi-invoke behavior can be observed at all, and a hang (rather than a refusal) burns the caller's timeout.
Fix: the Invoke handler's execution path. Open issue `20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md` describes the same refusal but does not carry these replay keys; the hang in alias-weight-ladder is additionally a timeout rather than the refusal that issue records.
Traces: lambda.cli-lambda-create-invoke, lambda.cli-lambda-invoke-nonexistent-error, lambda.alias-weight-ladder-observed-distribution.

Cause 4 of 6 - Container-image functions carry no CodeSha256.
AWS: UpdateFunctionCode/GetFunction for a PackageType=Image function report CodeSha256 (the image manifest digest), always present in FunctionConfiguration.
Emulator: the response has no CodeSha256 member at all - the check fails on `assert resp.get("CodeSha256")`.
Why: a client that pins or compares deployed image content has nothing to compare; CodeSha256 is a required part of the function configuration shape.
Fix: the UpdateFunctionCode/GetFunction serializer and the image code store for PackageType=Image.
Traces: lambda.container-image-update-code.

Cause 5 of 6 - UpdateFunctionCode with DryRun reports the wrong CodeSha256.
AWS: UpdateFunctionCode with DryRun=true validates the submitted package and reports the hash OF THE PACKAGE IT VALIDATED, while leaving the deployed code untouched.
Emulator: the dry run reports the hash of the currently deployed package instead of the submitted one (`the dry run must report the hash of the package it validated, got '7iKxFVAIjYQNorq/d6wzNagtLljNXD16kUec7OOAkK8='` - the value is per-run, the divergence is that it is the old package's digest).
Why: DryRun is the pre-deploy validation step; reporting the old digest makes it impossible to confirm which artifact was validated.
Fix: the UpdateFunctionCode handler's DryRun branch (hash the submitted package, do not persist it).
Traces: lambda.update-function-code-dry-run-changes-nothing-at-runtime.

Cause 6 of 6 - LoggingConfig with LogFormat=JSON does not default the log levels.
AWS: setting LogFormat=JSON alone defaults both ApplicationLogLevel and SystemLogLevel to INFO, and GetFunctionConfiguration reports them.
Emulator: LoggingConfig comes back as `{'LogFormat': 'JSON', 'LogGroup': '/aws/lambda/<fn>'}` with both level members absent.
Why: a client reading the effective log configuration cannot tell what is being emitted, and level-filtering behavior cannot be reasoned about.
Fix: LoggingConfig defaulting in the CreateFunction/UpdateFunctionConfiguration state and its serializer.
Traces: lambda.log-tail-python-json-levels-keep-python-level-names.

## Evidence
Cause 1: expected 200 with the handler's response; got `{'status': 404, 'headers': {'content-type': 'application/x-amz-json-1.1', 'x-amzn-errortype': 'UnknownService', ...}, 'body': b'{"__type":"UnknownService","message":"no emulated service is registered for \'lambda\'"}'}` (lambda.url-get-event-v2-..., lambda.stream-url-error-after-first-write-...); the SigV4 URL checks report `Error: Check timed out after 60.0s`.
Cause 2: expected a terminal Status; got `AssertionError: condition not met within 45.0s; last={... 'Status': 'RUNNING', 'DurableConfig': {'RetentionPeriodInDays': 1, 'ExecutionTimeout': 600}}`; callback checks report `Check timed out after 60.0s`.
Cause 3: `aws: [ERROR]: An error occurred (NotImplemented) when calling the Invoke operation: Invoke resolved arn:aws:lambda:us-west-2:000000000000:function:<fn> at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator...` (rc=254).
Cause 4: `assert resp.get("CodeSha256")` -> AssertionError (member absent).
Cause 5: `AssertionError: the dry run must report the hash of the package it validated, got '<digest of the previously deployed package>'`.
Cause 6: `AssertionError: LogFormat=JSON alone must default both levels to INFO: {'LogFormat': 'JSON', 'LogGroup': '/aws/lambda/<fn>'}`.

## Steps to Reproduce
1. git_clone(); 2. run_behavioral_suite(); 3. replay the check ids in trace_ids. For cause 1: create a function, CreateFunctionUrlConfig(AuthType=NONE), GET the returned FunctionUrl -> observe 404 UnknownService instead of the handler response. For cause 2: Invoke with X-Amz-Durable-Execution-Name, then poll GetDurableExecutionState -> Status stays RUNNING past 45s. For cause 3: `aws lambda invoke --function-name <fn> /dev/null`. For cause 4: create a PackageType=Image function, UpdateFunctionCode with a new image, read CodeSha256. For cause 5: UpdateFunctionCode(DryRun=true, ZipFile=<new package>) and compare the reported CodeSha256 with the submitted package's SHA-256. For cause 6: UpdateFunctionConfiguration(LoggingConfig={'LogFormat':'JSON'}) then GetFunctionConfiguration.
