Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__logs.log-cwl-missing-custom-log-group-is-created-on-first-invoke lambda__logs.log-cwl-custom-log-group-shared-by-two-functions-keeps-streams-apart lambda__logs.log-cwl-node-text-multiline-console-log-is-one-event
Issue-Key: lambda__logs.log-cwl-missing-custom-log-group-is-created-on-first-invoke|run#11|AssertionError|assert any(e["message"] == f"first-{m}\n" for e in events), (
Issue-Key: lambda__logs.log-cwl-custom-log-group-shared-by-two-functions-keeps-streams-apart|run#15|AssertionError|assert h == [stream], f"{fn}'s print must be in exactly its own stream {stream!r}, found in {h!r}"
Issue-Key: lambda__logs.log-cwl-node-text-multiline-console-log-is-one-event|run#14|AssertionError|assert len(fields) == 4 and re.fullmatch(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z", fields[0]), (
Issue-Commit: 117dc8980bd60242192cfb0322b38a4da9ec7838

# lambda__logs: 2 net-new causes — delivered log event messages drop their trailing newline, and the Node.js Text log format is not applied (no timestamp/request-id/level framing, multiline console.log split)

> _Note: this issue was identified against the code base at commit `117dc8980bd60242192cfb0322b38a4da9ec7838`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Log delivery from an invocation now reaches the function's CloudWatch Logs group (the previously filed open issue 20261107T035448Z-lambda-logs-3-net-new-causes-loggingconfig-loggroup.md carried these three check ids while they still timed out at `_poll_until`, and LoggingConfig.LogGroup is now honoured), but the delivered EVENT TEXT is not what AWS writes. Two distinct causes, both in how the invoke path frames stdout into log events.

Cause 1 of 2: every delivered log event message is missing its trailing newline.
AWS: Smithy (lambda) LoggingConfig.LogFormat selects "between plain text and structured JSON"; LoggingConfig.LogGroup (@length 1..512, @pattern ^[\.\-_/#A-Za-z0-9]+$) names the destination group. With LogFormat=Text, each line a handler writes to stdout is delivered to CloudWatch Logs as one event whose `message` is that line INCLUDING the terminating '\n', and the runtime's own records are likewise 'START RequestId: <request id> Version: $LATEST\n', 'END RequestId: <request id>\n' and 'REPORT RequestId: <request id>\tDuration: ... ms\t...\n'. The fixed text here is the newline terminator on every event message (and the START/END/REPORT framing words); the variable parts are the handler's own line content and the invocation's request id — never a literal from one run.
Emulator: the message is delivered with the newline stripped. For a python3.12 handler doing `print("first-" + marker)` the group holds exactly ['START RequestId: <id> Version: $LATEST', 'first-<marker>', 'END RequestId: <id>', 'REPORT RequestId: <id>\tDuration: 1101.47 ms\tBilled Duration: 1102 ms\tMemory Size: 128 MB'] — correct content, no terminator on any of the four. (The same REPORT line also still omits AWS's trailing '\tMax Memory Used: <n> MB' field, which open issue 20261107T035448Z already files as its cause 2; it is not re-filed here.)
Why: a client that reads log events back and matches a line it knows it printed (`message == "first-<marker>\n"`, or `"who-<marker>\n" in messages`) finds nothing, so log assertions, line-oriented log shippers, subscription-filter consumers and anything that re-joins events into a byte-exact stream all mis-handle output the function really produced.
Fix: the Lambda invoke path's log-delivery component that turns captured stdout/runtime records into PutLogEvents entries — it must not trim the line terminator.
Traces: lambda__logs.log-cwl-missing-custom-log-group-is-created-on-first-invoke (print found as 'first-<marker>' instead of 'first-<marker>\n'), lambda__logs.log-cwl-custom-log-group-shared-by-two-functions-keeps-streams-apart (the per-function print is found in NO stream, because the search is for the newline-terminated message).

Cause 2 of 2: the Node.js managed runtime's Text log format is not applied — console.log output carries no framing and its embedded newlines split it into separate events.
AWS: for the nodejs managed runtimes with LogFormat=Text, a `console.log(...)` is delivered as ONE CloudWatch Logs event framed '<ISO-8601 timestamp with milliseconds>\t<request id>\tINFO\t<message>\n' (four tab-separated fields; the level word is the console method's level), and newlines embedded INSIDE the logged string are preserved inside that single event rather than becoming new events. Raw `process.stdout.write("a\nb\n")` is the contrast case: it is NOT framed and arrives as one event per line. Fixed text: the tab separators and the level token; variable: the timestamp, the request id and the message.
Emulator: the console.log arrives as the bare string with no framing at all — the event holding the first line is exactly 'ML1-<marker>' (no timestamp field, no request id field, no 'INFO' field, no tab separators, no trailing newline), and the embedded '\nML2-...\nML3-...' is not kept in that event.
Why: every consumer of Lambda Node.js logs parses this framing — CloudWatch Logs Insights' discovered fields, metric filters over '[timestamp, request_id, level, message]' space/tab patterns, and any correlation of a log line to the invocation's request id. With the prefix gone the request id is not recoverable from the log line at all, and a multi-line message is no longer attributable to one logging call.
Fix: the Text-format framing in the Lambda runtime's log capture for the nodejs runtimes (prefix construction and the rule that a single console.* call is one event, while raw stdout writes split per line).
Traces: lambda__logs.log-cwl-node-text-multiline-console-log-is-one-event.

## Evidence
Cause 1 — expected the first invocation's print to be present as the event message 'first-<marker>\n'. Actual: AssertionError: /sim2real/sim2real-behav-l4-<id> must hold the first invocation's print: ['START RequestId: 6713859d-93bc-43f2-b33b-6078a88bd637 Version: $LATEST', 'first-m-4d4892ad8981', 'END RequestId: 6713859d-93bc-43f2-b33b-6078a88bd637', 'REPORT RequestId: 6713859d-93bc-43f2-b33b-6078a88bd637\tDuration: 1101.47 ms\tBilled Duration: 1102 ms\tMemory Size: 128 MB'] — the printed line is there but unterminated.
Cause 1 (second check) — expected each function's 'who-<marker>\n' in exactly its own stream. Actual: AssertionError: sim2real-behav-l4-256a84a67d11's print must be in exactly its own stream '2026/11/07/[$LATEST]f0cad3a44cff443788f15102607dbcfb', found in [].
Cause 2 — expected the console.log event to split on tab into 4 fields with field[0] matching \\d{4}-\\d{2}-\\d{2}T\\d{2}:\\d{2}:\\d{2}\\.\\d{3}Z and fields[1:] == [<request id>, 'INFO', 'ML1-<m>\nML2-<m>\nML3-<m>\n']. Actual: AssertionError: bad framing 'ML1-m-4ce12133d739' — a single untabbed, unterminated field.

## Steps to Reproduce
1. Create an IAM role trusted by lambda.amazonaws.com with AWSLambdaBasicExecutionRole.
2. Cause 1: lambda.create_function(Runtime='python3.12', LoggingConfig={'LogFormat':'Text','LogGroup':'/sim2real/<custom>'}, handler = `print("first-" + event["marker"])`); Invoke once; then logs.filter_log_events/get_log_events on that group and compare each event['message'] to the line the handler printed — the trailing '\n' is absent on the print and on START/END/REPORT.
3. Repeat with two functions sharing one LoggingConfig.LogGroup and search the returned messages for 'who-<marker>\n' — no stream matches.
4. Cause 2: lambda.create_function(Runtime='nodejs22.x', handler = `console.log("ML1-"+m+"\nML2-"+m+"\nML3-"+m); process.stdout.write("RAW1-"+m+"\nRAW2-"+m+"\n")`); Invoke, then read /aws/lambda/<fn> and inspect the event containing 'ML1-': it is the bare text with no '<timestamp>\t<request id>\tINFO\t' prefix and the ML2/ML3 continuation lines are not inside it.
