Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.start-query-end-time-before-start-time-rejected
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.start-query-end-time-before-start-time-rejected|run#14|AssertionError|assert msg == expected_msg, f"unexpected message: {msg!r}"
Issue-Commit: 6bf56903357d8bdb827814619137753aca334420

# logs: StartQuery's inverted-time-window refusal carries an emulator-authored message instead of AWS's InvalidParameterException wording (1 cause)

> _Note: this issue was identified against the code base at commit `6bf56903357d8bdb827814619137753aca334420`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQuery

## Description
One net-new root cause in the CloudWatch Logs surface, grounded in a failing behavioral check whose check id leads with `logs`. The check is not marked ALREADY FILED and not marked OWNED in the replay report, and no open issue carries it: the StartQuery causes already filed under 20261025T192833Z-logs-cloudwatch-logs-7-net-new-root-causes-startquery.md (dialect mismatch, HAVING-on-aggregate, mixed log class, MalformedQueryException token/position text) and under 20261025T192833Z-logs-reopens-the-cloudwatch-logs-startquery-createexporttask-fix-both.md (the `lookup` command, and CreateExportTask's from/to messages) are all different causes, and 20261026T102156Z-logs-cloudwatch-logs-export-tasks-complete-but-write-no.md covers a window ending before the log group was created, not an inverted window. This is NOT a REOPENS: the call is correctly refused here (AWS refuses it too) and the status and error code already match — only the message text diverges.

----------------------------------------------------------------
Cause 1 of 1: StartQuery's InvalidParameterException message for endTime < startTime is invented text, and it omits the AWS Java-SDK error envelope.
Failing check: logs.start-query-end-time-before-start-time-rejected

AWS: Smithy (services/service-com-amazonaws-cloudwatchlogs-logs-20140328 model): StartQuery.startTime and StartQuery.endTime are both @required, target Timestamp, @range(min 0) longs; nothing in the model orders them relative to each other, and StartQuery models InvalidParameterException (client error, HTTP 400; shape doc "A parameter is specified incorrectly.") alongside LimitExceededException, MalformedQueryException, ResourceNotFoundException and ServiceUnavailableException. Recorded behavior: a StartQuery whose endTime precedes its startTime is REFUSED with HTTP 400 / error code InvalidParameterException and the message
  "End time cannot be less than Start time (Service: AWSLogs; Status Code: 400; Error Code: InvalidParameterException; Request ID: <id>; Proxy: null)"
The fixed text of that message is "End time cannot be less than Start time" followed by the AWS Java-SDK error envelope "(Service: AWSLogs; Status Code: 400; Error Code: InvalidParameterException; Request ID: <id>; Proxy: null)". Within the envelope, Service (AWSLogs), Status Code (400), Error Code (InvalidParameterException) and "Proxy: null" are fixed; only the Request ID slot is a per-request value drawn from the response's own request id (the check masks it before comparing, so it is not part of the requirement). No part of the message echoes the submitted startTime/endTime values. The refused call also schedules nothing: DescribeQueries(logGroupName=<group>) afterwards reports `queries: []`.
Emulator: HTTP 400 with error code InvalidParameterException — the status and code already match — but the message is "End time should not be before start time." This is emulator-authored wording: a different sentence ("should not be before" instead of "cannot be less than", plus a trailing period) and no AWS Java-SDK envelope at all, so neither the fixed sentence nor the Service/Status Code/Error Code/Request ID/Proxy fields AWS appends are present.
Why: the error message is part of the error contract for this refusal class. A client (or an operator reading a log line) that matches, parses or surfaces AWS's wording for an inverted Logs Insights query window sees a sentence that exists nowhere in AWS, and loses the whole envelope — including the Request ID, which is the only handle AWS gives for correlating a refusal with a backend request. Because the Logs surface appends this envelope to every refusal, a message formatter that drops it diverges for this refusal and makes the emulator's refusals structurally unlike AWS's.
Fix: the InvalidParameterException message construction in the start-time/end-time validation of the StartQuery handler of the cloudwatch-logs crate (the StartQuery operation handler and the shared refusal/message formatter it renders through). Emit AWS's fixed sentence for this class and append the Java-SDK envelope with Service=AWSLogs, the response's own status code and error code, the per-request request id, and "Proxy: null"; do not interpolate request values into the sentence and do not hard-code any one run's request id.
Traces: logs.start-query-end-time-before-start-time-rejected.

## Evidence
Replay report (suite simparity-behavioral-ga-prod-v1, 173/393 passed; service `logs` 149/236):

[mismatch] logs.start-query-end-time-before-start-time-rejected
  File "check_logs_start_query_end_time_before_start_time_rejected.py", line 52, in run
    assert msg == expected_msg, f"unexpected message: {msg!r}"
  AssertionError: unexpected message: 'End time should not be before start time.'

Request: logs.StartQuery(logGroupName=<group>, startTime=<now_s>, endTime=<now_s - 3600>, queryString="fields @message") after CreateLogGroup + CreateLogStream + PutLogEvents on that group.

Expected (recorded AWS): HTTP 400, Error Code InvalidParameterException, Error Message
  "End time cannot be less than Start time (Service: AWSLogs; Status Code: 400; Error Code: InvalidParameterException; Request ID: <id>; Proxy: null)"
  — fixed text "End time cannot be less than Start time" + the Java-SDK envelope; the Request ID slot is per-request (the check masks it with the regex `Request ID: [0-9a-f-]+;` before comparing, so it is not asserted).
Actual (emulator): HTTP 400, Error Code InvalidParameterException (both correct), Error Message "End time should not be before start time." — different fixed sentence, no envelope.

The error code assertion immediately above the failing line (`code == "InvalidParameterException"`) passed, so the divergence is confined to the message. The check never reached its follow-on assertions (DescribeQueries(logGroupName=<group>) must report `queries: []`, and the positive control over a valid window must return exactly one matching record), so those remain ungraded behind this message defect.

## Steps to Reproduce
Boot the emulator from this checkout and replay the check id in trace_ids, or by hand:
1. logs.CreateLogGroup(logGroupName=<g>, tags={"Sim2RealOwner": "behavioral-suite"}); logs.CreateLogStream(logGroupName=<g>, logStreamName=<s>).
2. logs.PutLogEvents(logGroupName=<g>, logStreamName=<s>, logEvents=[{timestamp: <now_ms>, message: "hello <marker> world"}]).
3. logs.StartQuery(logGroupName=<g>, startTime=<now_s>, endTime=<now_s - 3600>, queryString="fields @message")
   -> expect HTTP 400 InvalidParameterException with message "End time cannot be less than Start time (Service: AWSLogs; Status Code: 400; Error Code: InvalidParameterException; Request ID: <this response's request id>; Proxy: null)"; observe "End time should not be before start time." with no envelope.
4. Then logs.DescribeQueries(logGroupName=<g>) -> expect `queries: []` (the refused call must schedule nothing).
5. Positive control: logs.StartQuery(logGroupName=<g>, startTime=<now_s - 3600>, endTime=<now_s + 60>, queryString='fields @message | filter @message like /<marker>/') -> expect 200 with a queryId, and GetQueryResults to reach status Complete with exactly one record whose @message contains <marker>.
