Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__logs.log-cwl-log-group-update-redirects-the-next-invocation lambda__logs.log-cwl-role-without-logs-permission-keeps-the-tail-and-creates-no-group lambda__logs.log-cwl-self-structured-json-lines-are-filtered-by-their-level-key lambda__logs.log-cwl-system-debug-level-delivers-runtime-done-and-init-records
Issue-Key: lambda__logs.log-cwl-log-group-update-redirects-the-next-invocation|_poll_until#8|AssertionError|raise AssertionError(f"{description} not observed within {timeout_s}s; last saw {last!r}")
Issue-Key: lambda__logs.log-cwl-role-without-logs-permission-keeps-the-tail-and-creates-no-group|_poll_until#11|AssertionError|raise AssertionError(f"{description} not observed within {timeout_s}s; last saw {last!r}")
Issue-Key: lambda__logs.log-cwl-self-structured-json-lines-are-filtered-by-their-level-key|_poll_until#13|AssertionError|raise AssertionError(f"{description} not observed within {timeout_s}s; last saw {last!r}")
Issue-Key: lambda__logs.log-cwl-system-debug-level-delivers-runtime-done-and-init-records|_poll_until#9|AssertionError|raise AssertionError(f"{description} not observed within {timeout_s}s; last saw {last!r}")
Issue-Commit: a5c62b28646ac18350a618623d28fc3117c37127

# 4 net-new causes in CloudWatch Logs delivery: LogGroup override ignored, second function's records missing, level filtering absent, JSON platform records absent

> _Note: this issue was identified against the code base at commit `a5c62b28646ac18350a618623d28fc3117c37127`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Four net-new root causes in the Lambda -> CloudWatch Logs delivery path. The open issue 20261107T143806Z-lambda-logs-2-net-new-causes-delivered-log.md covers other checks in this service; none of the four below carry its replay key.

Cause 1 of 4 — updating LoggingConfig.LogGroup does not redirect the next invocation's records, and the pre-update records are not where AWS puts them.
AWS: recorded behavior only. Before the update, an invocation's records land in the function's current log group; after UpdateFunctionConfiguration changes LoggingConfig.LogGroup, the next invocation's records land in the new group and none are added to the old one.
Emulator: the pre-update print is never found in the default group. Replay detail: `AssertionError: the pre-update print in /aws/lambda/sim2real-behav-l4-<hex> not observed within 150.0s; last saw [{'stream': '2026/11/07/[$LATEST]<hex>', 'message': 'START RequestId: ...'}, {... 'where-m-<hex>'}, {... 'END ...'}, {... 'REPORT ...'}]` — the group holds one invocation's records, not the pre-update one, so the before/after split LoggingConfig defines is not reproduced.
Why: a client that repoints a function's log group loses the history it expected in the old group and cannot tell which invocations went where, breaking log routing and retention policy per group.
Fix: the invoke log-delivery path's resolution of LoggingConfig.LogGroup per invocation (and the records it must write to the group in force at that time).
Traces: lambda__logs.log-cwl-log-group-update-redirects-the-next-invocation.

Cause 2 of 4 — only one of two functions' invocations is delivered; the twin's records never appear.
AWS: recorded behavior only. Two functions invoked in the same scenario each write their own records into their own `/aws/lambda/<function>` group; a role that lacks logs permissions does not get a group created for it, while its twin's records are still delivered.
Emulator: the twin's print never arrives. Replay detail: `AssertionError: the twin's print in /aws/lambda/sim2real-behav-l4-<hex> not observed within 150.0s; last saw [... 'out-m-<hex>' ...]`.
Why: a client cannot rely on logs being delivered per function — some functions' invocations simply produce no log events, so the absence of a log line cannot be read as the absence of an invocation.
Fix: the invoke log-delivery path — one delivery per function per invocation, with the execution role's logs permissions deciding only whether the group is created, not whether a granted twin is delivered.
Traces: lambda__logs.log-cwl-role-without-logs-permission-keeps-the-tail-and-creates-no-group.

Cause 3 of 4 — self-structured JSON log lines are not filtered by their level key.
AWS: recorded behavior only. With LoggingConfig.LogFormat=JSON and an ApplicationLogLevel, Lambda reads the `level` key of a handler-emitted JSON line and delivers only records at or above the configured level; lines with no level, or an unrecognised level, are treated per AWS's documented default.
Emulator: no filtering is applied — the group shows every line, including the debug/levelless/bogus ones the configured level excludes, and the check's expected warn/error-only view is never observed. Replay detail: `AssertionError: the warn and error objects in /aws/lambda/sim2real-behav-l4-<hex> not observed within 150.0s; last saw [... {"level": "debug", ...}, {"msg": ...}, {"level": "bogus", ...}, {"level": "warn", ...}, {"level": "error", ...} ...]`.
Why: a client that sets ApplicationLogLevel to cut log volume and cost sees every record delivered anyway, and cannot distinguish "the handler did not log at this level" from "the level filter is not applied".
Fix: the application-log filtering stage of log delivery — parse the JSON record's level key and apply LoggingConfig.ApplicationLogLevel.
Traces: lambda__logs.log-cwl-self-structured-json-lines-are-filtered-by-their-level-key.

Cause 4 of 4 — JSON platform records (platform.report, init records) are never emitted.
AWS: recorded behavior only. With LogFormat=JSON and SystemLogLevel=DEBUG, Lambda delivers structured platform records — `platform.initStart`, `platform.runtimeDone`, `platform.report` — alongside the application records.
Emulator: only the text framing is delivered. Replay detail: `AssertionError: platform.report for <requestId> in /aws/lambda/sim2real-behav-l4-<hex> not observed within 150.0s; last saw [{'message': 'START RequestId: ... Version: $LATEST'}, {'message': 'END RequestId: ...'}, {'message': 'REPORT RequestId: ...\tDuration: 1151.60 ms\tBilled Duration: 1152 ms\tMemory Size: 128 MB'}]` — plain text lines, no JSON platform records.
Why: a client parsing structured platform records for billed duration, init duration or runtimeDone status gets text lines it cannot parse, and SystemLogLevel has no observable effect.
Fix: the platform-record emitter in log delivery — emit platform.initStart / platform.runtimeDone / platform.report as JSON records when LogFormat=JSON, gated by SystemLogLevel.
Traces: lambda__logs.log-cwl-system-debug-level-delivers-runtime-done-and-init-records.

## Evidence
Cause 1: expected the pre-update invocation's print in the function's then-current group and the post-update one in the new group; actual the pre-update print is never found (`not observed within 150.0s`).
Cause 2: expected both functions' prints in their own /aws/lambda/<fn> groups; actual the twin's print never observed within 150 s.
Cause 3: expected only the warn and error JSON objects delivered under the configured ApplicationLogLevel; actual all records (debug, levelless, bogus, warn, error) present.
Cause 4: expected JSON platform records `platform.report` (and init/runtimeDone) under LogFormat=JSON + SystemLogLevel=DEBUG; actual only text START/END/REPORT lines.

## Steps to Reproduce
Cause 1: Invoke a function, then UpdateFunctionConfiguration with LoggingConfig.LogGroup=<new group>, Invoke again, and read both groups.
Cause 2: Invoke two functions in the same scenario (one role with logs permissions, one without) and read each /aws/lambda/<fn> group.
Cause 3: Set LoggingConfig {LogFormat: JSON, ApplicationLogLevel: WARN}; have the handler print JSON lines at debug/warn/error plus one with no level and one with a bogus level; read the group.
Cause 4: Set LoggingConfig {LogFormat: JSON, SystemLogLevel: DEBUG}; Invoke and read the group for platform.* records.
