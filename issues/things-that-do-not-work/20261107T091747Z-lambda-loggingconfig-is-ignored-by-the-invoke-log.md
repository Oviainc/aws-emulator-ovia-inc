Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.log-tail-node-text-console-lines-are-tab-framed lambda.log-tail-node-json-console-methods-map-to-their-levels lambda.log-lambda-report-json-format-platform-and-application-records-field-by-field lambda.log-tail-node-json-error-argument-adds-error-fields lambda.log-tail-node-json-object-argument-stays-structured lambda.log-tail-node-json-unhandled-error-nests-the-error-in-message lambda.log-tail-node-json-error-level-drops-lower-console-records lambda.log-tail-application-log-level-update-admits-debug-on-next-invoke lambda.log-lambda-report-system-log-level-filters-platform-records-independently lambda.log-tail-init-phase-output-precedes-first-start-only lambda.invoke-log-result lambda.log-lambda-report-log-result-tail-is-last-4kb-of-invocation-log lambda.log-lambda-report-start-end-report-lines-bracket-the-invoke-request-id lambda.log-lambda-report-stream-name-format-carries-date-and-version-qualifier lambda.log-lambda-report-new-environment-opens-new-stream-warm-reuse-shares-one lambda.log-lambda-report-unhandled-exception-error-line-matches-function-error-and-tail lambda.log-lambda-report-unpublished-version-invoke-rejected-and-writes-no-stream
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.log-tail-node-text-console-lines-are-tab-framed|run#16|AssertionError|assert got == want, f"framed (level, message) must be {want!r}; got {got!r}"
Issue-Key: lambda.log-tail-node-json-console-methods-map-to-their-levels|run#11|AssertionError|assert got == want, f"console records (level, message) must be {want!r}; got {got!r}"
Issue-Key: lambda.log-lambda-report-json-format-platform-and-application-records-field-by-field|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: lambda.log-tail-node-json-error-argument-adds-error-fields|run#9|AssertionError|assert len(app) == 1, f"expected one application record; tail {tail!r}"
Issue-Key: lambda.log-tail-node-json-object-argument-stays-structured|run#9|AssertionError|assert len(app) == 2, f"expected two application records; tail {tail!r}"
Issue-Key: lambda.log-tail-node-json-unhandled-error-nests-the-error-in-message|run#9|AssertionError|assert len(app) == 1, f"expected one application record for the throw; tail {tail!r}"
Issue-Key: lambda.log-tail-node-json-error-level-drops-lower-console-records|run#7|AssertionError|assert body == {"level": "ERROR"}, f"the runtime must see AWS_LAMBDA_LOG_LEVEL=ERROR: {body!r}"
Issue-Key: lambda.log-tail-application-log-level-update-admits-debug-on-next-invoke|run#8|AssertionError|assert body1 == {"effective": 20}, f"at INFO the root logger must be INFO (20): {body1!r}"
Issue-Key: lambda.log-lambda-report-system-log-level-filters-platform-records-independently|run#8|AssertionError|assert {"platform.start", "platform.report"} <= _tail_types(tail_a), (
Issue-Key: lambda.log-tail-init-phase-output-precedes-first-start-only|run#10|AssertionError|assert l1.index("INIT-index") < start1, f"init output must precede START: {tail1!r}"
Issue-Key: lambda.invoke-log-result|run#18|Timeout|time.sleep(2.0)
Issue-Key: lambda.log-lambda-report-log-result-tail-is-last-4kb-of-invocation-log|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: lambda.log-lambda-report-start-end-report-lines-bracket-the-invoke-request-id|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: lambda.log-lambda-report-stream-name-format-carries-date-and-version-qualifier|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: lambda.log-lambda-report-new-environment-opens-new-stream-warm-reuse-shares-one|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: lambda.log-lambda-report-unhandled-exception-error-line-matches-function-error-and-tail|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: lambda.log-lambda-report-unpublished-version-invoke-rejected-and-writes-no-stream|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Commit: 677b6e35c5226b67b9bfea5e554d250e3c084a9c

# lambda: LoggingConfig is ignored by the invoke log pipeline — no level framing, no JSON LogFormat, no log-level filtering, init output misordered, and the LogType=Tail checks never complete (6 net-new causes)

> _Note: this issue was identified against the code base at commit `677b6e35c5226b67b9bfea5e554d250e3c084a9c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Six net-new root causes in how an invocation's log records are produced and filtered. The emulator does accept and echo LoggingConfig, so the control plane claims the configuration is in effect.

Cause 1 of 6: application log lines are not framed with a level.
AWS: Smithy models LoggingConfig.LogFormat (enum Text|JSON) and ApplicationLogLevel/SystemLogLevel enums. With LogFormat=Text, AWS's managed Node runtime writes each console record as a tab-framed line carrying the level token (console.trace/debug/info/log/warn/error -> TRACE/DEBUG/INFO/INFO/WARN/ERROR), which is what a client parses to recover the level.
Emulator: the framed (level, message) list recovered from the log stream is empty — the records carry no level framing at all.
Why: a client cannot tell an ERROR line from an INFO line, so log-based alerting and the documented level semantics are unobservable.
Fix: the invoke log pipeline's record framing for the Node runtime.
Traces: lambda.log-tail-node-text-console-lines-are-tab-framed, lambda.log-tail-node-json-console-methods-map-to-their-levels.

Cause 2 of 6: LogFormat=JSON produces no structured records.
AWS: LoggingConfig.LogFormat=JSON makes Lambda emit each application and platform record as a JSON object (with timestamp/level/requestId/message fields, and error fields nested for an Error argument), per the LoggingConfig documentation quoted above ("Select between plain text and structured JSON").
Emulator: the stream still carries plain text — a console.error with an Error argument appears as the raw message plus an untagged stack, an object argument appears as Node's inspect form ("{ marker: '...', data: 12.3, ... }"), and an unhandled throw appears as the Text-format "<ts>\t<requestId>\tERROR\tInvoke Error \t{...}" line — so zero JSON application records are found.
Why: a client that selects JSON logging to parse records programmatically gets unparseable text.
Fix: the invoke log pipeline's serializer, keyed off LoggingConfig.LogFormat.
Traces: lambda.log-lambda-report-json-format-platform-and-application-records-field-by-field, lambda.log-tail-node-json-error-argument-adds-error-fields, lambda.log-tail-node-json-object-argument-stays-structured, lambda.log-tail-node-json-unhandled-error-nests-the-error-in-message.

Cause 3 of 6: ApplicationLogLevel is neither propagated to the runtime nor used to filter.
AWS: ApplicationLogLevel filters application logs to the selected level and lower; the managed runtimes see it as AWS_LAMBDA_LOG_LEVEL and configure the language logger accordingly (at INFO the Python root logger's effective level is INFO/20).
Emulator: the Node runtime sees AWS_LAMBDA_LOG_LEVEL unset (level None) and the Python root logger stays at WARNING (effective 30) when ApplicationLogLevel=INFO, so lower-level records are neither admitted nor dropped as configured.
Why: a client cannot raise or lower function log verbosity; debug output configured through the API never appears, and records that should be dropped are still billed and stored.
Fix: reserved-environment composition for the invoke (AWS_LAMBDA_LOG_LEVEL) plus level filtering in the log pipeline.
Traces: lambda.log-tail-node-json-error-level-drops-lower-console-records, lambda.log-tail-application-log-level-update-admits-debug-on-next-invoke.

Cause 4 of 6: SystemLogLevel does not filter platform records independently.
AWS: SystemLogLevel filters the platform (START/END/REPORT and platform.* ) records independently of ApplicationLogLevel.
Emulator: the control leg shows the platform records were emitted unfiltered — the tail is 'START RequestId: <id> Version: $LATEST\n<app line>\nEND RequestId: <id>\nREPORT RequestId: <id>\tDuration: ... ms\tBilled Duration: ... ms\tMemory Size: 128 MB\n' regardless of SystemLogLevel (the fixed text is AWS's START/END/REPORT wording; the request id, durations and memory size come from the invocation).
Why: a client cannot suppress platform noise while keeping application logs, or vice versa.
Fix: level filtering for platform records in the invoke log pipeline.
Traces: lambda.log-lambda-report-system-log-level-filters-platform-records-independently.

Cause 5 of 6: init-phase output is emitted after the START record.
AWS: output written during init precedes the first START line for that environment, which is how a client attributes module-scope output to the cold start.
Emulator: the tail is 'START RequestId: <id> Version: $LATEST\nINIT-index\nHANDLER-<marker>\nEND ...' — the init line lands after START.
Why: cold-start diagnosis from logs is wrong; init output is attributed to the invocation.
Fix: record ordering/phase attribution in the invoke log pipeline.
Traces: lambda.log-tail-init-phase-output-precedes-first-start-only.

Cause 6 of 6: the LogType=Tail / per-stream log checks never complete.
AWS: Invoke with LogType=Tail returns X-Amz-Log-Result (modeled output member LogResult) carrying the last 4 KB of the invocation log, and every invocation writes a stream under /aws/lambda/<function name> whose name carries the date and version qualifier.
Emulator: recorded behavior only — each of these checks is cut off by the suite's 60 s budget ("Error: Check timed out after 60.0s"), so the LogResult tail, the START/END/REPORT bracketing, the stream-name format, the new-stream-per-environment rule, the unhandled-exception error line and the "unpublished version writes no stream" rule are all unserved rather than wrong. Hypothesis: the same log pipeline never produces the awaited records, so the checks poll to the deadline.
Why: a client polling logs or reading LogResult after an invoke waits forever instead of getting the records AWS guarantees.
Fix: the invoke log pipeline and LogResult (X-Amz-Log-Result) population in the Invoke handler.
Traces: lambda.invoke-log-result, lambda.log-lambda-report-log-result-tail-is-last-4kb-of-invocation-log, lambda.log-lambda-report-start-end-report-lines-bracket-the-invoke-request-id, lambda.log-lambda-report-stream-name-format-carries-date-and-version-qualifier, lambda.log-lambda-report-new-environment-opens-new-stream-warm-reuse-shares-one, lambda.log-lambda-report-unhandled-exception-error-line-matches-function-error-and-tail, lambda.log-lambda-report-unpublished-version-invoke-rejected-and-writes-no-stream.

## Evidence
Expected vs actual, from the replay:
- log-tail-node-text-console-lines-are-tab-framed: expected framed (level, message) [('INFO','L-..'),('WARN','W-..'),('ERROR','E-..'),('INFO','ML1-..'),('INFO',"{ marker: '..', n: 1 }")]; got []
- log-tail-node-json-console-methods-map-to-their-levels: expected [('TRACE',..),('DEBUG',..),('INFO',..),('INFO',..),('WARN',..),('ERROR',..)]; got []
- log-tail-node-json-error-argument-adds-error-fields: expected one JSON application record; tail contained the raw message plus an untagged stack
- log-tail-node-json-object-argument-stays-structured: expected two application records; tail contained Node inspect text "{ marker: '..', data: 12.3, flag: false, list: [ 1, 'two' ] }"
- log-tail-node-json-unhandled-error-nests-the-error-in-message: expected one JSON record for the throw; tail contained the Text-format "<ts>\t<id>\tERROR\tInvoke Error \t{...}" line
- log-tail-node-json-error-level-drops-lower-console-records: AssertionError: the runtime must see AWS_LAMBDA_LOG_LEVEL=ERROR: {'level': None}
- log-tail-application-log-level-update-admits-debug-on-next-invoke: AssertionError: at INFO the root logger must be INFO (20): {'effective': 30}
- log-lambda-report-system-log-level-filters-platform-records-independently: AssertionError: control: the tail must show the platform records WERE emitted ... 'START RequestId: ... END ... REPORT ...'
- log-tail-init-phase-output-precedes-first-start-only: AssertionError: init output must precede START: 'START RequestId: ...\nINIT-index\nHANDLER-...'
- invoke-log-result, log-lambda-report-json-format-..., log-lambda-report-log-result-tail-..., log-lambda-report-start-end-report-lines-..., log-lambda-report-stream-name-format-..., log-lambda-report-new-environment-opens-new-stream-..., log-lambda-report-unhandled-exception-error-line-..., log-lambda-report-unpublished-version-invoke-rejected-...: Error: Check timed out after 60.0s

## Steps to Reproduce
1. CreateFunction with LoggingConfig={LogFormat:'Text', ApplicationLogLevel:'INFO', SystemLogLevel:'WARN'} (nodejs and python3.x variants) and a handler that emits console.trace/debug/info/log/warn/error (or logging at several levels), writes during init, and throws on demand.
2. Invoke with LogType='Tail'; read X-Amz-Log-Result and FilterLogEvents on /aws/lambda/<function name>.
3. Repeat with UpdateFunctionConfiguration LoggingConfig.LogFormat='JSON' and ApplicationLogLevel='ERROR'.
4. Observe unframed/plain-text records, no JSON records, AWS_LAMBDA_LOG_LEVEL unset in the runtime, unfiltered platform records, init output after START, and the Tail/stream checks never settling.
5. Re-replay the check ids in trace_ids.
