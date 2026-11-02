Issue-Source: trace-sweep-divergence
Issue-Trace: logs-insights-start-query-and-describe-workflow
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-insights-start-query-and-describe-workflow logs-log-group-query-discovery
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|StartQuery|4xx->4xx|$.message
Issue-Commit: 9aea9ab22874a078ce52e9235c6d97813056f831

# logs StartQuery MalformedQueryException message diverges from the recorded syntax-error refusal text

> _Note: this issue was identified against the code base at commit `9aea9ab22874a078ce52e9235c6d97813056f831`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQuery

## Description
One net-new root cause in the CloudWatch Logs surface. Cause count: 1.

=== Cause 1 of 1: the MalformedQueryException `message` StartQuery returns for a syntactically invalid query string does not match the refusal text the recording holds for that error class ===

AWS:      Smithy (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/smithy-model/com-amazonaws-cloudwatchlogs-logs-20140328.json) models StartQuery with `queryString` @required and MalformedQueryException among its errors (errorKind client, httpError 400, httpErrorDefaulted), carrying `message` (Message) and `queryCompileError` (QueryCompileError{location{startCharOffset,endCharOffset}, message}). The model does not state the query grammar or the refusal wording — for the message itself: recorded behavior only. The recording refuses the query synchronously with HTTP 400 and error code MalformedQueryException (both of which the emulator already matches) and carries a `message` that the corpus holds as a SHAPED value rather than a literal (it replays as `<shaped-not-asserted>`), i.e. the recorded text is a template: the fixed text `unexpected symbol found ` + the offending token taken from the submitted queryString + ` at line 1 and position ` + the position of that token, and AWS additionally appends its Java-SDK envelope `(Service: AWSLogs; Status Code: 400; Error Code: MalformedQueryException; Request ID: <id>; Proxy: null)`. The requirement on the number is that it TRACKS the offending token: AWS reports the offset within the statement the SERVICE parses, which is the caller's string prefixed by a service-rewritten SOURCE clause, so between two queries differing only in the bad token's offset the reported positions must differ by exactly that offset. The emulator's value does not satisfy the recorded shape.

Emulator: HTTP 400 with error code MalformedQueryException and `message` = `unexpected symbol found abc at line 1 and position 24` — the status and the code are right, only the message value diverges from the recorded template. In that string the fixed parts are `unexpected symbol found ` and ` at line 1 and position `; `abc` is the offending token taken from the request (correct source), and `24` is the position. Hypothesis: the position is computed as the raw character offset in the caller's own queryString, with no allowance for the SOURCE clause the service prepends before parsing, so the number does not track the offset AWS reports for the same token; the Java-SDK envelope suffix is also absent. Nothing in the model or the recording states which of the two it is, hence Hypothesis.

Why:      the `message` is the only channel that tells a client WHERE its query broke, and a client that parses or surfaces AWS's refusal text (or asserts the reported position against its own query offsets) reads a position that does not correspond to the one AWS reports for the same token. Query-editor and query-builder clients that underline the offending span from the reported position point at the wrong characters, and a client matching AWS's full refusal sentence (envelope included) does not match at all, so a refusal AWS classifies precisely degrades into an unparseable one.

Fix:      the MalformedQueryException message construction in the queryString parse/validation stage of the StartQuery handler, cloudwatch-logs crate (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/start_query.rs) — the token must keep coming from the submitted query string (it already does) while the reported position must be computed in the same frame of reference AWS parses in, and the same offsets must populate queryCompileError.location.startCharOffset/endCharOffset. Related open issues that do NOT cover this cause and do NOT carry this fixture/step as a replay key: 20261025T192833Z-logs-cloudwatch-logs-7-net-new-root-causes-startquery.md (its cause 4 is the earlier state where the message was the model's doc sentence `The query string is not valid.`, which the emulator no longer returns) and 20261101T213737Z-logs-startquery-accepts-a-query-with-a-dangling.md (its cause 1 is the missing end-of-input `<EOF>` branch, i.e. an ACCEPTED query, not a wrong message on a refused one). Neither addresses the value of the position in an already-correct refusal.

Traces:   logs-insights-start-query-and-describe-workflow#task#4 and logs-log-group-query-discovery#task#6 are the two recorded steps of this one cluster (divergence_key service=logs|StartQuery|status=400->400|keys=$.message).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (per-run markers, generated names and request ids inside the quotes are not part of the requirement):

[mismatch] service=logs operation=StartQuery
  fixture logs-insights-start-query-and-describe-workflow step task#4 (representative)
  also affected: logs-log-group-query-discovery step task#6
  divergence_key: service=logs|StartQuery|status=400->400|keys=$.message
  expected_status 400, actual_status 400  (status and error class already agree)
  detail: $.message: value_mismatch (expected '<shaped-not-asserted>', got 'unexpected symbol found abc at line 1 and position 24')

Expected: HTTP 400, MalformedQueryException, `message` matching the recorded template for a syntax fault — fixed text `unexpected symbol found ` + the offending token from the submitted queryString + ` at line 1 and position ` + that token's position in the statement the service parses, with AWS's `(Service: AWSLogs; Status Code: 400; Error Code: MalformedQueryException; Request ID: <id>; Proxy: null)` envelope appended. The corpus holds the recorded message as a shaped (non-literal) value, so the literal recorded sentence is not asserted; the emulator's value fails that shape.
Actual:   HTTP 400, MalformedQueryException, `message` = 'unexpected symbol found abc at line 1 and position 24'. Token `abc` is request-derived and correct; the position `24` and the missing envelope are what diverge.

Model backing (smithy_query on services/service-com-amazonaws-cloudwatchlogs-logs-20140328/smithy-model/com-amazonaws-cloudwatchlogs-logs-20140328.json):
  MalformedQueryException: errorKind client, httpError 400 (defaulted), members { message: Message, queryCompileError: QueryCompileError }
  QueryCompileError: { location: QueryCompileErrorLocation{ startCharOffset: integer, endCharOffset: integer }, message: Message }
  StartQuery.queryString: @required string; the grammar and the refusal wording are not modeled.

All 73 divergence clusters in this replay are service=logs; this is the only one with an empty `owned_by` and an empty `stub_path`.

## Steps to Reproduce
1. Boot the emulator from this checkout (replay_wire_corpus reports emulator_up with /__admin/health 200).
2. logs.CreateLogGroup(logGroupName=<g>); logs.CreateLogStream(logGroupName=<g>, logStreamName=<s>); logs.PutLogEvents with one event carrying a marker.
3. logs.StartQuery(logGroupNames=[<g>], startTime=<t0>, endTime=<t1>, queryString=<a Logs Insights pipeline whose last command is an unknown token, e.g. "fields @timestamp, @message | abc">).
4. Observe HTTP 400 with Error.Code 'MalformedQueryException' (correct) and Message 'unexpected symbol found abc at line 1 and position 24'. The recording's message for this error class is the same template but with the position AWS reports for that token in the statement the service parses, plus AWS's '(Service: AWSLogs; Status Code: 400; Error Code: MalformedQueryException; Request ID: <id>; Proxy: null)' suffix.
5. Control for the position requirement: issue the same query with the offending token shifted by N characters (pad the preceding filter) and check that the reported position moves by exactly N.
6. Replay the wire fixture steps logs-insights-start-query-and-describe-workflow#task#4 and logs-log-group-query-discovery#task#6 to confirm the fix.
