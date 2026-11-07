Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.init-past-the-init-limit-is-retried-inside-the-invoke-and-timed-out-rejected lambda.timeout-update-moves-the-enforced-deadline-rejected-past-it lambda.layer-direct-upload-request-over-size-limit-rejected-no-version-minted lambda.log-lambda-report-log-result-tail-is-last-4kb-of-invocation-log lambda.log-lambda-report-new-environment-opens-new-stream-warm-reuse-shares-one lambda.log-lambda-report-start-end-report-lines-bracket-the-invoke-request-id lambda.log-lambda-report-stream-name-format-carries-date-and-version-qualifier lambda.log-lambda-report-unhandled-exception-error-line-matches-function-error-and-tail lambda.log-lambda-report-unpublished-version-invoke-rejected-and-writes-no-stream lambda.log-lambda-report-json-format-platform-and-application-records-field-by-field lambda.logging-config-log-group-override-routes-logs-and-leaves-the-default-group-absent lambda.published-version-keeps-its-own-timeout-enforcement-rejected-past-it lambda.url-deleted-config-stops-serving-the-url lambda.url-none-auth-grants-to-a-named-principal-do-not-admit-anonymous-callers
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.init-past-the-init-limit-is-retried-inside-the-invoke-and-timed-out-rejected|run#13|AssertionError|assert body.get("errorMessage") == want, (
Issue-Key: lambda.timeout-update-moves-the-enforced-deadline-rejected-past-it|_assert_timed_out#8|AssertionError|assert body.get("errorMessage") == want, (
Issue-Key: lambda.layer-direct-upload-request-over-size-limit-rejected-no-version-minted|run#10|AssertionError|assert code == "RequestEntityTooLargeException", (
Issue-Key: lambda.log-lambda-report-log-result-tail-is-last-4kb-of-invocation-log|_stream_of#15|AssertionError|assert len(holders) == 1, f"{line!r} must be in exactly one stream, found in {holders!r}"
Issue-Key: lambda.log-lambda-report-new-environment-opens-new-stream-warm-reuse-shares-one|_stream_of#16|AssertionError|assert len(holders) == 1, f"{line!r} must be in exactly one stream, found in {holders!r}"
Issue-Key: lambda.log-lambda-report-start-end-report-lines-bracket-the-invoke-request-id|_stream_of#13|AssertionError|assert len(holders) == 1, f"{line!r} must be in exactly one stream, found in {holders!r}"
Issue-Key: lambda.log-lambda-report-stream-name-format-carries-date-and-version-qualifier|_stream_of#19|AssertionError|assert len(holders) == 1, f"{line!r} must be in exactly one stream, found in {holders!r}"
Issue-Key: lambda.log-lambda-report-unhandled-exception-error-line-matches-function-error-and-tail|_stream_of#17|AssertionError|assert len(holders) == 1, f"{line!r} must be in exactly one stream, found in {holders!r}"
Issue-Key: lambda.log-lambda-report-unpublished-version-invoke-rejected-and-writes-no-stream|_stream_of#15|AssertionError|assert len(holders) == 1, f"{line!r} must be in exactly one stream, found in {holders!r}"
Issue-Key: lambda.log-lambda-report-json-format-platform-and-application-records-field-by-field|_poll_until#11|Timeout|raise AssertionError(f"{description} not observed within {timeout_s}s; last saw {last!r}")
Issue-Key: lambda.logging-config-log-group-override-routes-logs-and-leaves-the-default-group-absent|_poll_until#17|AssertionError|raise AssertionError(f"{description} not observed within {timeout_s}s; last saw {last!r}")
Issue-Key: lambda.published-version-keeps-its-own-timeout-enforcement-rejected-past-it|run#11|AssertionError|assert isinstance(entry, int) and (_V1_TIMEOUT_S - 1) * 1000 < entry <= _V1_TIMEOUT_S * 1000, (
Issue-Key: lambda.url-deleted-config-stops-serving-the-url|_poll_until#12|Timeout|raise AssertionError(f"{description} not observed within {timeout_s}s; last saw {last!r}")
Issue-Key: lambda.url-none-auth-grants-to-a-named-principal-do-not-admit-anonymous-callers|_poll_until#8|AssertionError|raise AssertionError(f"{description} not observed within {timeout_s}s; last saw {last!r}")
Issue-Commit: a5c62b28646ac18350a618623d28fc3117c37127

# 6 net-new causes: timeout error text, oversize PublishLayerVersion, invoke logs never reach a log stream, per-version timeout, function-URL deletion and none-auth

> _Note: this issue was identified against the code base at commit `a5c62b28646ac18350a618623d28fc3117c37127`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Six further net-new root causes in the Lambda service, distinct from the five bundled in the companion issue for this service.

Cause 1 of 6 — the task-timeout error message is not AWS's.
AWS: recorded behavior only. When an invocation exceeds the function's Timeout, Lambda answers with FunctionError 'Unhandled' and errorType `Sandbox.Timedout`, and the errorMessage is the fixed text `RequestId: <requestId> Error: Task timed out after <N>.<NN> seconds` — the request id and the seconds come from the invocation and the function's Timeout; everything else is fixed.
Emulator: the errorMessage drops the `RequestId: ` and ` Error:` framing. Replay details: `AssertionError: errorMessage '<requestId> Task timed out after 3.00 seconds', expected 'RequestId: <requestId> Error: Task timed out after 3.00 seconds'` and the same divergence after a Timeout update.
Why: a client that parses or greps the timeout message (a very common way to separate timeouts from other unhandled errors in logs and alarms) never matches, so timeouts are misclassified.
Fix: the invoke timeout path's error-payload serializer — the errorMessage template for Sandbox.Timedout.
Traces: lambda.init-past-the-init-limit-is-retried-inside-the-invoke-and-timed-out-rejected, lambda.timeout-update-moves-the-enforced-deadline-rejected-past-it.

Cause 2 of 6 — an oversize direct-upload PublishLayerVersion answers a bare 413 with no error body.
AWS: recorded behavior only. PublishLayerVersion with Content.ZipFile above the direct-upload limit is refused with HTTP 413 and a modelled error, `RequestEntityTooLargeException`, in the body; Smithy models PublishLayerVersion at POST /2018-10-31/layers/{LayerName}/versions with success code 201 and client errors CodeStorageExceededException / InvalidParameterValueException, and AWS additionally returns the platform-wide 413 RequestEntityTooLargeException for an oversize request. No layer version is minted.
Emulator: the status is right but there is no error at all. Replay detail: `AssertionError: expected a 52887792-byte direct-upload PublishLayerVersion request to be rejected with RequestEntityTooLargeException, got 413: ''` — an empty body, so no `__type`/code and no message.
Why: every SDK turns a 413 with an empty body into a generic transport error instead of RequestEntityTooLargeException, so a client cannot catch the modelled exception, cannot retry correctly, and gets no message naming the limit.
Fix: the PublishLayerVersion request-size guard and its error serializer — emit the RequestEntityTooLargeException error shape (code and message) with the 413.
Traces: lambda.layer-direct-upload-request-over-size-limit-rejected-no-version-minted.

Cause 3 of 6 — invocation log lines are never written to any CloudWatch Logs stream.
AWS: recorded behavior only. Every invocation writes its `START RequestId: <id> Version: <qualifier>` / handler output / `END` / `REPORT` lines into exactly one stream of the function's log group, named `<YYYY>/<MM>/<DD>/[<qualifier>]<hex>`; a warm reuse shares a stream and a new environment opens a new one. The same records are what LogResult tails.
Emulator: the lines exist nowhere. Replay details (six checks): `AssertionError: 'START RequestId: <id> Version: $LATEST\n' must be in exactly one stream, found in []`, `AssertionError: 'ran-real-<hex>\n' must be in exactly one stream, found in []`, plus `Error: Check timed out after 180.0s` for the JSON-format record comparison. A LoggingConfig.LogGroup override is equally empty: `AssertionError: a JSON record carrying 'marker-custom-<hex>' arriving in /sim2real/behav/logroute-<hex> not observed within 90.0s; last saw None`.
Why: a client has no durable record of any invocation — no START/END/REPORT framing, no handler output, no stream naming or per-environment grouping, and no honouring of a custom log group. Everything built on Lambda logs (debugging, metric filters, subscription filters, log-based alarms) sees an empty group.
Fix: the Lambda invoke path's CloudWatch Logs delivery — creation of the per-environment log stream with AWS's name format, and emission of the platform (START/END/REPORT) and application records into the function's configured LoggingConfig.LogGroup.
Traces: lambda.log-lambda-report-log-result-tail-is-last-4kb-of-invocation-log, lambda.log-lambda-report-new-environment-opens-new-stream-warm-reuse-shares-one, lambda.log-lambda-report-start-end-report-lines-bracket-the-invoke-request-id, lambda.log-lambda-report-stream-name-format-carries-date-and-version-qualifier, lambda.log-lambda-report-unhandled-exception-error-line-matches-function-error-and-tail, lambda.log-lambda-report-unpublished-version-invoke-rejected-and-writes-no-stream, lambda.log-lambda-report-json-format-platform-and-application-records-field-by-field, lambda.logging-config-log-group-override-routes-logs-and-leaves-the-default-group-absent.

Cause 4 of 6 — a published version does not enforce its own publish-time Timeout.
AWS: recorded behavior only. A published version freezes its configuration: an invocation of version N is bounded by the Timeout that version was published with, independently of later changes to $LATEST, so `get_remaining_time_in_millis()` at handler entry reflects that version's Timeout.
Emulator: the version runs under a different, smaller deadline. Replay detail: `AssertionError: version 1 entered with 1976 ms; expected its own Timeout=3`.
Why: a client that publishes a version to pin behaviour gets an invocation cut short (or extended) by the wrong deadline, so a version is not actually immutable and long handlers fail only on the versioned alias.
Fix: the invoke path's resolution of per-version configuration — the frozen Timeout of the qualified version must set the invocation deadline and the remaining-time budget.
Traces: lambda.published-version-keeps-its-own-timeout-enforcement-rejected-past-it.

Cause 5 of 6 — deleting a function URL config never stops the URL serving.
AWS: recorded behavior only. After DeleteFunctionUrlConfig, requests to the function URL stop being served (the endpoint no longer answers the handler); the recording polls for that transition.
Emulator: the check never reaches a verdict — `Error: Check timed out after 180.0s`, so the URL never stops serving within the window.
Why: a client that revokes a public function URL as a containment action finds the endpoint still live, with no way to tell when (or whether) the revocation takes effect.
Fix: the function-URL data plane's lookup of the URL config on each request, so a deleted config stops routing.
Traces: lambda.url-deleted-config-stops-serving-the-url.

Cause 6 of 6 — a function URL with AuthType NONE admits/refuses anonymous callers on the wrong rule.
AWS: recorded behavior only. For AuthType NONE, anonymous access requires a resource-policy statement granting `lambda:InvokeFunctionUrl` to the `*` principal; a grant to a *named* principal does not admit an anonymous caller, which is refused consistently.
Emulator: the refusals are not produced consistently over the window. Replay detail: `AssertionError: three consecutive authorizer refusals not observed within 60.0s; last saw {'status': 403, ..., 'body': b'{"Message":"Forbidden"}'}` — a 403 with body `{"Message":"Forbidden"}` is seen but not stably for the three consecutive probes the rule requires.
Why: a client cannot rely on the authorization decision for a NONE-auth URL being deterministic — the same anonymous request is sometimes admitted and sometimes refused, so a public endpoint's exposure is non-deterministic.
Fix: the function-URL authorizer — evaluation of the function's resource policy for lambda:InvokeFunctionUrl against an anonymous principal, made deterministic per request.
Traces: lambda.url-none-auth-grants-to-a-named-principal-do-not-admit-anonymous-callers.

## Evidence
Cause 1: expected errorMessage `RequestId: <requestId> Error: Task timed out after 3.00 seconds`; actual `<requestId> Task timed out after 3.00 seconds`.
Cause 2: expected 413 carrying RequestEntityTooLargeException; actual `got 413: ''` (empty body).
Cause 3: expected the START/END/REPORT and handler lines of each invocation in exactly one stream of the function's log group (stream name `YYYY/MM/DD/[qualifier]hex`), and a LoggingConfig.LogGroup override to receive them; actual `found in []` for every record and `last saw None` for the custom group.
Cause 4: expected version 1's own Timeout=3 to bound the invocation; actual the handler entered with 1976 ms remaining.
Cause 5: expected requests to stop being served after DeleteFunctionUrlConfig; actual `Error: Check timed out after 180.0s`.
Cause 6: expected three consecutive 403 refusals for an anonymous caller when the policy grants lambda:InvokeFunctionUrl only to a named principal; actual the 403 (body {"Message":"Forbidden"}) is not observed consistently within 60 s.

## Steps to Reproduce
Cause 1: CreateFunction with Timeout=3 and a handler that sleeps 4 s; Invoke and read the error payload's errorMessage.
Cause 2: PublishLayerVersion with Content.ZipFile larger than the direct-upload limit; inspect the 413 response body — it is empty.
Cause 3: Invoke a function that prints a marker, then DescribeLogStreams/GetLogEvents on /aws/lambda/<fn> — no stream and no START/END/REPORT records. Repeat with LoggingConfig.LogGroup set to a custom group.
Cause 4: CreateFunction with Timeout=3, PublishVersion, then UpdateFunctionConfiguration on $LATEST; Invoke version 1 and read get_remaining_time_in_millis() at entry.
Cause 5: CreateFunctionUrlConfig, confirm it serves, DeleteFunctionUrlConfig, then keep requesting the URL.
Cause 6: CreateFunctionUrlConfig with AuthType=NONE and AddPermission granting lambda:InvokeFunctionUrl to a named principal only; issue repeated unsigned requests.
