Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.e2e-lookup-table-enrichment-flow logs.export-task-honors-time-window-boundaries logs.e2e-export-task-rejected-for-invalid-input
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: cloudwatch-logs|StartQuery|MalformedQueryException
Issue-Key: logs.export-task-honors-time-window-boundaries|_create_export_task_with_retry#20|AssertionError|raise AssertionError(f"CreateExportTask never accepted the request: {last_err}")
Issue-Key: logs.e2e-export-task-rejected-for-invalid-input|run#13|AssertionError|assert message == (
Issue-Commit: 8c118d1d43e9fad6226f410894186130d2b1ac53

# REOPENS the CloudWatch Logs StartQuery/CreateExportTask fix: both operations now answer, but refuse calls real AWS accepts or refuse them with invented messages (3 causes)

> _Note: this issue was identified against the code base at commit `8c118d1d43e9fad6226f410894186130d2b1ac53`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQuery

## Description
REOPENS 20261025T070938Z-logs-cloudwatch-logs-6-net-new-root-causes-kinesis.md: its fix left the call refused.

That issue's Cause 2 (StartQuery) and Cause 3 (CreateExportTask) replaced the NotImplemented stub answers with real handlers, and one of its own replay keys (logs.e2e-export-task-rejected-for-invalid-input) is still failing. Both operations now answer, but each refuses a call that real AWS accepts, or refuses it with a message the emulator authored. These are not new defects; they are the un-finished part of that fix. 3 causes, all in service `logs`.

----------------------------------------------------------------
Cause 1 of 3: StartQuery refuses a valid CWLI query that uses the `lookup` command.
Failing check: logs.e2e-lookup-table-enrichment-flow

AWS: Smithy: StartQuery.queryString is @required, target QueryString @length(0..10000) — an opaque string; queryLanguage is enum QueryLanguage [CWLI, SQL, PPL]; modeled errors are InvalidParameterException, LimitExceededException, MalformedQueryException, ResourceNotFoundException (all 400) and ServiceUnavailableException (500). MalformedQueryException's model doc is "The query string is not valid. Details about this error are displayed in a QueryCompileError object." The `lookup <table> <key> OUTPUT <fields>` command is a documented CWLI command (CWL_QuerySyntax-Lookup), so a CWLI query of the form `fields <f...> | lookup <lookupTableName> <key> OUTPUT <fields>` over a log group, after CreateLookupTable has accepted the table, is ACCEPTED: 200 with a queryId, and GetQueryResults then returns rows carrying the joined columns.
Emulator: StartQuery(queryLanguage="CWLI", logGroupName=<g>, queryString="fields user_id, action, name, department | lookup <table> user_id OUTPUT name, department") is refused 400 MalformedQueryException with message "The query string is not valid." — note the message is verbatim the first sentence of the model's own MalformedQueryException doc, i.e. the handler's parser does not recognise `lookup` and falls back to the shape's documentation text.
Why: lookup-table enrichment is unreachable end to end. A client that creates a lookup table (CreateLookupTable succeeds here) and then queries through it cannot run a single query; the refusal also tells it its query is malformed when the query is valid, so the only corrective action it can take is to remove a command AWS supports.
Fix: the queryString parser/compiler behind the StartQuery handler of the cloudwatch-logs crate (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/start_query.rs) must accept the `lookup` command and resolve it against lookup-table state rather than rejecting it. Related open issue that does not carry this replay key: 20261023T113357Z-logs-startquery-stub.md.
Traces: logs.e2e-lookup-table-enrichment-flow.

----------------------------------------------------------------
Cause 2 of 3: CreateExportTask invents a precondition — it refuses an export window that ends before the log group was created.
Failing check: logs.export-task-honors-time-window-boundaries

AWS: Smithy: CreateExportTask.from and .to are both @required longs @range(min 0); modeled errors are InvalidParameterException, LimitExceededException, OperationAbortedException, ResourceAlreadyExistsException, ResourceNotFoundException (400) and ServiceUnavailableException (500). Nothing in the model ties `from`/`to` to the log group's creationTime, and no modeled refusal names that relation. Recorded behavior: an export whose [from, to) window lies wholly in the past relative to the group's creation is ACCEPTED (200 with a taskId) and exports exactly the events whose timestamp falls in the window — the check backdates its three planted events by 1-3 hours precisely because that is what real AWS accepts and exports.
Emulator: 400 InvalidParameterException with message "The parameter to must not be earlier than when the log group was created." — an emulator-authored sentence; the request is retried to exhaustion and never accepted ("CreateExportTask never accepted the request").
Why: a client exporting backdated or bulk-ingested events — the normal case for events written with explicit historical timestamps — can never create an export task at all, and the error blames a parameter that AWS considers valid. The invented message also cannot be matched by any client error handling written against AWS.
Fix: remove the creation-time precondition from the `from`/`to` validation in the CreateExportTask handler of the cloudwatch-logs crate (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/create_export_task.rs); the window is a filter over event timestamps, not a constraint against the group's creationTime.
Traces: logs.export-task-honors-time-window-boundaries.

----------------------------------------------------------------
Cause 3 of 3: CreateExportTask's inverted-time-range refusal carries an emulator-authored message.
Failing check: logs.e2e-export-task-rejected-for-invalid-input

AWS: Smithy: CreateExportTask models InvalidParameterException (400, shape doc "A parameter is specified incorrectly.") and the check's recording shows the status/code pair 400 InvalidParameterException for a to < from window. AWS's message for this error class is: fixed text "Specified time range of '" ... "' and '" ... "' is not valid. Please make sure the values are within the retention period of the log groups and from value is lesser than the to value", with the two quoted slots filled from the REQUEST — the `from` value then the `to` value as sent. The code already matches; only the message diverges.
Emulator: 400 InvalidParameterException with message "The parameter to must be greater than the parameter from." — fixed text invented by the emulator, naming no values from the request.
Why: the message is part of the error contract. A client (or a human reading a log) that matches or reports AWS's wording sees text that exists nowhere in AWS, and loses the echoed from/to values that identify which window was rejected.
Fix: the InvalidParameterException message template in the time-range validation of the CreateExportTask handler, cloudwatch-logs crate (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/create_export_task.rs). Reproduce AWS's fixed text and interpolate the request's own from/to values; do not hard-code the values from any one recording.
Traces: logs.e2e-export-task-rejected-for-invalid-input (this check id is also in the Issue-Trace-Ids of the fixed issue this one reopens).

## Evidence
Replay report (suite simparity-behavioral-ga-prod-v1, 124/432 passed; service `logs` 113/244):

Cause 1 — [error] logs.e2e-lookup-table-enrichment-flow
  botocore.errorfactory.MalformedQueryException: An error occurred (MalformedQueryException) when calling the StartQuery operation: The query string is not valid.
  Request: StartQuery(queryLanguage="CWLI", logGroupName=<group>, startTime=<now-600s>, endTime=<now+600s>,
           queryString="fields user_id, action, name, department | lookup <lookup_table_name> user_id OUTPUT name, department")
  Expected: 200 {"queryId": <id>}, and GetQueryResults rows carrying name/department joined from the lookup table
            (u1 -> Alice/Engineering, u2 -> Bob/Sales for the CSV the check uploads).
  Actual:   400 MalformedQueryException, message "The query string is not valid."

Cause 2 — [mismatch] logs.export-task-honors-time-window-boundaries
  AssertionError: CreateExportTask never accepted the request: An error occurred (InvalidParameterException) when calling the CreateExportTask operation: The parameter to must not be earlier than when the log group was created.
  Request: CreateExportTask(logGroupName=<group created seconds ago>, destination=<bucket>, destinationPrefix=<prefix>,
           from=<t_in - 30min>, to=<t_in + 30min>) where t_in = now - 2h (the three planted events are at now-3h, now-2h, now-1h).
  Expected: 200 {"taskId": <id>}, task reaches COMPLETED, and the exported object contains only the in-window event.
  Actual:   400 InvalidParameterException "The parameter to must not be earlier than when the log group was created." on every retry.

Cause 3 — [mismatch] logs.e2e-export-task-rejected-for-invalid-input
  AssertionError: unexpected CreateExportTask message for an inverted time range: 'The parameter to must be greater than the parameter from.'
  Request: CreateExportTask(logGroupName=<existing group>, destination=<bucket>, from=<now_ms>, to=<now_ms - 60000>)
  Expected: 400 InvalidParameterException, message "Specified time range of '<from>' and '<to>' is not valid. Please make sure the values are within the retention period of the log groups and from value is lesser than the to value" (fixed text as quoted; <from>/<to> are the request's own values).
  Actual:   400 InvalidParameterException, message "The parameter to must be greater than the parameter from."

## Steps to Reproduce
Boot the emulator from this checkout and replay the three `logs` checks in trace_ids.

Cause 1 (StartQuery `lookup`):
  1. logs.CreateLookupTable(lookupTableName=<t>, tableBody="user_id,name,department\nu1,Alice,Engineering\nu2,Bob,Sales\n").
  2. logs.CreateLogGroup + CreateLogStream; PutLogEvents of JSON lines carrying user_id/action.
  3. logs.StartQuery(queryLanguage="CWLI", logGroupName=<g>, startTime=<t0>, endTime=<t1>,
     queryString="fields user_id, action, name, department | lookup <t> user_id OUTPUT name, department")
     -> expect 200 with a queryId; observe 400 MalformedQueryException "The query string is not valid."

Cause 2 (CreateExportTask creation-time precondition):
  1. logs.CreateLogGroup + CreateLogStream.
  2. logs.PutLogEvents with timestamps 1-3 hours in the PAST.
  3. s3.CreateBucket (+ bucket policy allowing logs.<region>.amazonaws.com GetBucketAcl/PutObject).
  4. logs.CreateExportTask(logGroupName=<g>, destination=<bucket>, from=<now-2.5h>, to=<now-1.5h>)
     -> expect 200 with a taskId; observe 400 InvalidParameterException "The parameter to must not be earlier than when the log group was created."

Cause 3 (inverted-range message):
  1. logs.CreateLogGroup; s3.CreateBucket as above.
  2. logs.CreateExportTask(logGroupName=<g>, destination=<bucket>, from=<now_ms>, to=<now_ms-60000>)
     -> expect 400 InvalidParameterException with AWS's "Specified time range of '<from>' and '<to>' is not valid. ..." wording; observe "The parameter to must be greater than the parameter from."
