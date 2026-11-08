Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.logsquery-parse-glob-extracts-fields-from-unstructured-lines logs.logsquery-display-projects-only-the-named-fields logs.logsquery-sort-on-a-parsed-number-orders-numerically logs.logsquery-stats-group-key-absent-for-events-without-the-field logs.logs-insights-semantics-stats-aggregates-by-field-match-hand-computation logs.logs-insights-semantics-sort-limit-dedup-display-arithmetic logs.logs-insights-semantics-stop-running-query-reports-cancelled logs.stop-query-terminal-state-rejected logs.deep-logs-1-logsql-sql-group-by-having-matches-hand-computation logs.deep-logs-1-logsql-ppl-backticked-fields-where-stats-match-hand-computation logs.deep-logs-2-logcmk-query-result-key-gates-insights
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.logsquery-parse-glob-extracts-fields-from-unstructured-lines|run#19|AssertionError|assert (got.get("svc"), got.get("lat"), got.get("st")) == (want_svc, want_lat, want_st), (
Issue-Key: logs.logsquery-display-projects-only-the-named-fields|run#20|AssertionError|assert set(row) == {"svc", "lat", "st", "@ptr"}, (
Issue-Key: logs.logsquery-sort-on-a-parsed-number-orders-numerically|run#19|AssertionError|assert got_desc == numeric_desc, (
Issue-Key: logs.logsquery-stats-group-key-absent-for-events-without-the-field|run#19|AssertionError|assert rows[2] == {"n": "1"}, (
Issue-Key: logs.logs-insights-semantics-stats-aggregates-by-field-match-hand-computation|run#13|AssertionError|assert resp["status"] == "Complete", f"stats query ended {resp['status']!r}"
Issue-Key: cloudwatch-logs|StartQuery|MalformedQueryException
Issue-Key: cloudwatch-logs|StopQuery|InvalidParameterException
Issue-Key: logs.stop-query-terminal-state-rejected|run#21|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: logs.deep-logs-1-logsql-sql-group-by-having-matches-hand-computation|_await#14|AssertionError|assert seen[-1] == "Complete", (
Issue-Key: logs.deep-logs-1-logsql-ppl-backticked-fields-where-stats-match-hand-computation|_query_until_scanned#10|Timeout|time.sleep(3)
Issue-Key: logs.deep-logs-2-logcmk-query-result-key-gates-insights|run#29|AssertionError|assert both == "ValidationException", (
Issue-Commit: 544316519d780db35004270636d68199c524c41e

# logs: 9 net-new causes — CloudWatch Logs Insights projection, stats, SQL/PPL dialects, StopQuery state machine and AssociateKmsKey selector validation all diverge

> _Note: this issue was identified against the code base at commit `544316519d780db35004270636d68199c524c41e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQuery, GetQueryResults, StopQuery, AssociateKmsKey

## Description
Nine net-new root causes in the CloudWatch Logs query surface, reached by 11 behavioral checks. All nine must be closed before this issue is submitted; each cause below names the failing check ids it is grounded in.

Cause 1 of 9 — an Insights pipeline with no explicit projection drops the fields `parse` extracted.
AWS: Smithy (logs) GetQueryResults returns `results` as a list of rows of ResultField {field,value}; the row's columns are the pipeline's output columns. Recorded behavior: for a pipeline `filter @message like /<marker>/ | parse @message "svc=* lat=* status=*" as svc, lat, st | sort @timestamp asc`, every matching row carries exactly the parsed field names plus the engine-generated `@ptr`, and carries NO `@message`/`@timestamp`, because the pipeline never projects one; a line the parse pattern does not match is still returned as a row whose only field is `@ptr` (the parsed keys are absent, not empty strings and not nulls). The same holds when the parse uses a named-capture regex (`parse @message /lat=(?<lat>\d+)ms/`), and `sort <parsed-field>` then orders the parsed values NUMERICALLY (300,200,100,25,9 — not the lexicographic 9,300,25,200,100).
Emulator: the same pipelines come back with the default columns only — rows are `['@message','@ptr','@timestamp']` and `row.get("svc")`/`lat`/`st` are all None, so `sort lat desc` yields `[None,None,None,None,None]`. The parsed columns appear ONLY when a later `display`/`fields` names them (the `| display svc, lat` leg of the same check passes), i.e. parse results are not added to the row's field set, they are merely resolvable by an explicit projection.
Why: every client that parses unstructured lines and reads the extracted fields off the rows (the canonical Insights idiom, and what the console shows) gets no data at all, and any sort on a parsed field is meaningless.
Fix: the Insights query engine's row-projection/field-resolution stage for the `parse` command — parsed fields must enter the row's output column set, and the implicit default projection must be replaced once the pipeline produces its own columns.
Traces: logs.logsquery-parse-glob-extracts-fields-from-unstructured-lines, logs.logsquery-display-projects-only-the-named-fields (its control leg, without `display`), logs.logsquery-sort-on-a-parsed-number-orders-numerically.

Cause 2 of 9 — `stats ... by <field>` invents an empty-string group key for events that lack the field.
AWS: recorded behavior only (the model types a row as ResultField {field,value} and says nothing about grouping). Events that have no value for the grouping field aggregate into ONE bucket whose row OMITS the grouping key entirely — the field is simply absent from that row, so the row is `{"n": "<count>"}`.
Emulator: that bucket's row is `{"svc": "", "n": "1"}` — the grouping key is present with an empty-string value.
Why: a client keying rows by the grouping field sees a spurious group named "" and cannot distinguish "field absent" from "field present and empty"; code that does `row["svc"]` silently attributes those events to a real-looking empty service.
Fix: the aggregation/row-serialization stage of the Insights `stats ... by` implementation — omit the key member for the keyless bucket instead of emitting an empty value.
Traces: logs.logsquery-stats-group-key-absent-for-events-without-the-field.

Cause 3 of 9 — a multi-aggregate `stats` query ends `Failed` instead of `Complete`.
AWS: Smithy (logs) QueryStatus enum = Scheduled|Running|Complete|Failed|Cancelled|Timeout|Unknown; a well-formed query over an existing log group reaches `Complete` and returns one row per `by` bucket. Recorded behavior: `stats count(*) as n, sum(ms) as s, avg(ms) as a, min(ms) as mn, max(ms) as mx, count_distinct(user) as u, pct(ms, 50) as p50 by svc | sort svc asc` completes and `pct` uses nearest-rank; a `stats` with no `by` returns exactly one row.
Emulator: the query reaches terminal status `Failed` (a bare `stats count(*) as n` over the same group in the same check completes, so the group and the window are fine). The emulator supplies no reason member, so the client only sees `Failed`.
Why: a client's dashboard/aggregation query returns no rows and no diagnosable error; `count_distinct` and `pct` are documented Insights aggregates, so any real query using them is unusable.
Fix: the Insights aggregate-function implementations behind StartQuery/GetQueryResults (notably `count_distinct` and `pct`) and the executor's error path, which must not mark a supported query `Failed`.
Traces: logs.logs-insights-semantics-stats-aggregates-by-field-match-hand-computation.

Cause 4 of 9 — the Insights query parser rejects the modulo operator in field arithmetic.
AWS: Smithy (logs) StartQuery models MalformedQueryException for a query it cannot parse; `+ - * / %` are all valid Insights arithmetic operators, so `fields id, ms * 2 + 1 as x, ms / 4 as y, ms % 7 as z | sort id asc` is accepted and StartQuery returns a queryId.
Emulator: StartQuery answers HTTP 400 MalformedQueryException with the message `unexpected symbol found ms at line 1 and position 41 (Service: AWSLogs; Status Code: 400; Error Code: MalformedQueryException; Request ID: <per-call id>; Proxy: null)` — position 41 is the start of the `ms % 7` term, i.e. `%` is not in the operator grammar. The fixed text here is `unexpected symbol found ` / ` at line ` / ` and position ` plus the service suffix; the symbol, line and position are taken from the request, and the Request ID is per-call.
Why: a valid query is refused outright, so the whole pipeline (sort/limit/dedup/display plus arithmetic) cannot run; the client sees a parse error for a documented operator.
Fix: the Insights query-language grammar/expression evaluator used by StartQuery — add `%` to the arithmetic operators.
Traces: logs.logs-insights-semantics-sort-limit-dedup-display-arithmetic.

Cause 5 of 9 — StopQuery on an already-Cancelled query is refused instead of being idempotent.
AWS: Smithy (logs) StopQuery input is `queryId`, output is `success` (boolean); its modeled client errors are InvalidParameterException and ResourceNotFoundException. Recorded behavior: stopping a query that is already `Cancelled` returns HTTP 200 with `success=true` and leaves the query `Cancelled` (only stopping a query that ENDED, e.g. `Complete`, is refused).
Emulator: the second StopQuery on the same, already-Cancelled query raises HTTP 400 InvalidParameterException with message `Query with id [<queryId>] is not running.`
Why: a client that stops a query defensively (retry, cleanup path, double-click in a console) gets a hard 400 for a no-op that AWS accepts, and cannot tell a genuine bad request from an idempotent re-stop.
Fix: the StopQuery handler's query-state machine — treat `Cancelled` as a success (idempotent) terminal state and reserve the refusal for queries that ended.
Traces: logs.logs-insights-semantics-stop-running-query-reports-cancelled.

Cause 6 of 9 — the StopQuery refusal for an ended query carries the wrong message.
AWS: recorded behavior for the modeled InvalidParameterException on StopQuery of a query that already ended: HTTP 400, error code InvalidParameterException, message `Query is already ended with <status> (Service: AWSLogs; Status Code: 400; Error Code: InvalidParameterException; Request ID: <request id>; Proxy: null)`. Fixed text: `Query is already ended with ` and the ` (Service: AWSLogs; Status Code: 400; Error Code: InvalidParameterException; Request ID: ...; Proxy: null)` suffix; the variable parts are the query's terminal status (from service state, e.g. `Complete`) and the per-call request id.
Emulator: the same refusal carries the code InvalidParameterException but the message `Query with id [<queryId>] is not running.` — different fixed text, no status, and none of the service/status-code/error-code/request-id suffix CloudWatch Logs appends.
Why: clients and SDK/CLI users match on this message to tell "already finished" from other InvalidParameterException causes; the divergent wording breaks that and hides which terminal status the query reached.
Fix: the StopQuery handler's refusal construction (message formatting for the ended-query case).
Traces: logs.stop-query-terminal-state-rejected.

Cause 7 of 9 — a SQL-dialect StartQuery with GROUP BY/HAVING ends `Failed`.
AWS: Smithy (logs) StartQuery accepts `queryLanguage` from the QueryLanguage enum CWLI|SQL|PPL. Recorded behavior: `SELECT svc, COUNT(*) AS n, SUM(ms) AS total FROM \`<log-group>\` WHERE ms > 5 GROUP BY svc HAVING n > 1 ORDER BY svc` (log group named only in FROM) reaches `Complete` and returns the aggregated rows, with `statistics.recordsMatched` counting the events the WHERE kept.
Emulator: the query's status trajectory is `['Scheduled', 'Failed']` — it never runs, and no reason is reported.
Why: the whole SQL query language is unusable for aggregation; a client issuing a supported SQL query gets a terminal `Failed` with nothing to act on.
Fix: the SQL dialect of the Insights query executor behind StartQuery/GetQueryResults (FROM-only log-group binding, GROUP BY, HAVING, ORDER BY) and its statistics accounting.
Traces: logs.deep-logs-1-logsql-sql-group-by-having-matches-hand-computation.

Cause 8 of 9 — a PPL-dialect StartQuery never scans the log group's events.
AWS: recorded behavior for `queryLanguage="PPL"`: `source = \`logGroups(logGroupIdentifier: ['<group>'])\` | where \`@logStream\` = 'a' and \`status-code\` = 500 | stats count() as n, sum(ms) as total by svc` completes with `statistics.recordsScanned` equal to the number of events in the window and `recordsMatched` equal to the number the `where` kept; backticked identifiers resolve hyphenated JSON keys and `@logStream`, while the unbackticked `status-code` is not a field reference and selects nothing.
Emulator: the check re-issues the query until `recordsScanned` equals the 8 planted events and never converges — the check is killed at the suite's 180 s ceiling with the PPL query still not reporting the planted events as scanned. Nothing else in the check ran.
Why: PPL queries silently return nothing (no error, no rows, no scan accounting), so a client cannot tell an empty log group from an unimplemented dialect.
Fix: the PPL dialect of the Insights query executor behind StartQuery/GetQueryResults — `source = logGroups(...)` binding, backticked identifier resolution, and the recordsScanned/recordsMatched statistics.
Traces: logs.deep-logs-1-logsql-ppl-backticked-fields-where-stats-match-hand-computation.

Cause 9 of 9 — AssociateKmsKey with both selectors is refused with the wrong error code and message.
AWS: Smithy (logs) AssociateKmsKey takes `kmsKeyId` (required) plus the two mutually exclusive selectors `logGroupName` and `resourceIdentifier`; the model does not enumerate ValidationException for this operation, so the recorded refusal governs: passing BOTH selectors is a request-shape violation answered with HTTP 400, error code `ValidationException`, message `resourceIdentifier should not be specified with any other log group attributes` (entirely fixed text), and nothing is associated.
Emulator: the same call is refused with error code `InvalidParameterException`.
Why: a client branching on the error code treats a request-shape violation as a parameter-value problem, and SDK/CLI error-type matching (ValidationException vs InvalidParameterException are different botocore exception classes) fails outright.
Fix: AssociateKmsKey's input validation — the both-selectors check must raise ValidationException with that message.
Traces: logs.deep-logs-2-logcmk-query-result-key-gates-insights (the first assertion reached; the rest of the query-result-CMK scenario is unmeasured behind it).

Related open issues that do NOT carry these replay keys and do not cover these causes: 20261025T192833Z-logs-cloudwatch-logs-7-net-new-root-causes-startquery.md and 20261102T090835Z-logs-cloudwatch-logs-insights-query-refusal-messages-diverge-missing.md (Insights refusal messages), 20261029T182816Z-logs-startquery-and-eight-other-logs-operations-accept.md (StartQuery accepting what AWS refuses), 20261103T105454Z-logs-associatekmskey-accepts-a-kmskeyid-that-names-no.md (AssociateKmsKey key existence, not selector exclusivity).

## Evidence
Cause 1 — Expected: `filter ... | parse @message "svc=* lat=* status=*" as svc, lat, st | sort @timestamp asc` rows carry {svc, lat, st, @ptr} (and the unparseable line carries {@ptr} only). Actual: row 0 fields were ['@message','@ptr','@timestamp']; `(got.get("svc"), got.get("lat"), got.get("st"))` == (None, None, None); `sort lat desc` over the parsed field returned [None, None, None, None, None] instead of ['300','200','100','25','9'].

Cause 2 — Expected: the keyless bucket row is {"n": "1"}. Actual: {"svc": "", "n": "1"}.

Cause 3 — Expected: `stats count(*) as n, sum(ms) as s, avg(ms) as a, min(ms) as mn, max(ms) as mx, count_distinct(user) as u, pct(ms, 50) as p50 by svc | sort svc asc` status == 'Complete'. Actual: AssertionError: stats query ended 'Failed'.

Cause 4 — Expected: StartQuery accepts `fields id, ms * 2 + 1 as x, ms / 4 as y, ms % 7 as z | sort id asc`. Actual: botocore.errorfactory.MalformedQueryException: An error occurred (MalformedQueryException) when calling the StartQuery operation: unexpected symbol found ms at line 1 and position 41 (Service: AWSLogs; Status Code: 400; Error Code: MalformedQueryException; Request ID: <per-call id>; Proxy: null).

Cause 5 — Expected: a second StopQuery on an already-Cancelled query returns success=True and leaves it Cancelled. Actual: botocore.errorfactory.InvalidParameterException: An error occurred (InvalidParameterException) when calling the StopQuery operation: Query with id [<queryId>] is not running.

Cause 6 — Expected message: 'Query is already ended with Complete (Service: AWSLogs; Status Code: 400; Error Code: InvalidParameterException; Request ID: <request id>; Proxy: null)'. Actual message: 'Query with id [<queryId>] is not running.'

Cause 7 — Expected: SQL GROUP BY/HAVING query reaches Complete with rows [{svc:api,n:3,total:60},{svc:db,n:2,total:300}] and recordsMatched 7.0. Actual: AssertionError: query <queryId> ended 'Failed' (trajectory ['Scheduled', 'Failed']).

Cause 8 — Expected: the PPL query reports recordsScanned == 8.0 (the planted events) and the hand-computed per-svc buckets. Actual: the re-query loop never saw recordsScanned reach 8; the check was killed at the 180 s ceiling with no PPL result.

Cause 9 — Expected: AssociateKmsKey(logGroupName=<group>, resourceIdentifier=arn:aws:logs:<region>:<account>:query-result:*, kmsKeyId=<key>) -> HTTP 400 ValidationException, message 'resourceIdentifier should not be specified with any other log group attributes'. Actual: error code 'InvalidParameterException'.

## Steps to Reproduce
Common setup: create a log group, create a stream, PutLogEvents with the events each cause needs, then query.

1. (Cause 1) Put lines of the shape `REQ <marker> svc=<s> lat=<n> status=<c>` plus one non-matching line; StartQuery `filter @message like /<marker>/ | parse @message "svc=* lat=* status=*" as svc, lat, st | sort @timestamp asc`; read GetQueryResults rows. Expected fields {svc,lat,st,@ptr} per row and {@ptr} for the unmatched line; observed ['@message','@ptr','@timestamp']. Repeat with `| parse @message /lat=(?<lat>\d+)ms/ | sort lat desc` and observe lat == None.
2. (Cause 2) Put some events with a `svc` field and some without; StartQuery `stats count(*) as n by svc`. Expected a keyless row {"n":"1"}; observed {"svc":"","n":"1"}.
3. (Cause 3) StartQuery `stats count(*) as n, sum(ms) as s, avg(ms) as a, min(ms) as mn, max(ms) as mx, count_distinct(user) as u, pct(ms, 50) as p50 by svc | sort svc asc`; poll GetQueryResults. Expected Complete; observed Failed.
4. (Cause 4) StartQuery `fields id, ms * 2 + 1 as x, ms / 4 as y, ms % 7 as z | sort id asc`. Expected a queryId; observed MalformedQueryException naming position 41 (`ms % 7`).
5. (Cause 5/6) StartQuery a long-running query, StopQuery it (success=True), poll GetQueryResults to Cancelled, then StopQuery again. Expected success=True; observed InvalidParameterException 'Query with id [...] is not running.' Separately, StopQuery a query that already reached Complete and compare the message with 'Query is already ended with Complete (Service: AWSLogs; ...)'.
6. (Cause 7) StartQuery with queryLanguage='SQL' and `SELECT svc, COUNT(*) AS n, SUM(ms) AS total FROM \`<group>\` WHERE ms > 5 GROUP BY svc HAVING n > 1 ORDER BY svc`; poll GetQueryResults. Expected Complete + rows; observed Scheduled -> Failed.
7. (Cause 8) StartQuery with queryLanguage='PPL' and `source = \`logGroups(logGroupIdentifier: ['<group>'])\` | where \`@logStream\` = 'a' and \`status-code\` = 500 | stats count() as n, sum(ms) as total by svc`; read statistics.recordsScanned. Expected the number of events in the window; observed it never reaches it.
8. (Cause 9) Call AssociateKmsKey with logGroupName AND resourceIdentifier=arn:aws:logs:<region>:<account>:query-result:* plus a kmsKeyId. Expected ValidationException 'resourceIdentifier should not be specified with any other log group attributes'; observed InvalidParameterException.
