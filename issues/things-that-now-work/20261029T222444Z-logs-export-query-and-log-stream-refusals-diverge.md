Issue-Source: trace-sweep-divergence
Issue-Trace: logs-export-task-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-export-task-lifecycle logs-insights-start-query-and-describe-workflow logs-log-group-query-discovery logs-logs-insights-query-workflow logsrd-describe-log-streams-golden logsval-describe-log-streams-order-by-last-event-time-with-prefix logsrd-get-log-events-out-of-window-read logs-aggregate-log-group-summaries-by-datasource logs-aggregate-log-group-summaries-with-name-pattern logs-list-aggregate-log-group-summaries-invalid-pattern
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|CreateExportTask|4xx->2xx|$.__type
Issue-Key: logs|CreateExportTask|4xx->2xx|$.message
Issue-Key: logs|CreateExportTask|4xx->2xx|status
Issue-Key: logs|StartQuery|4xx->4xx|$.message
Issue-Key: logs|StopQuery|4xx->4xx|$.message
Issue-Key: logs|DeleteQueryDefinition|4xx->4xx|$.message
Issue-Key: logs|DescribeLogStreams|4xx->4xx|$.message
Issue-Key: logs|GetLogEvents|4xx->4xx|$.message
Issue-Key: logs|ListAggregateLogGroupSummaries|2xx->2xx|$
Issue-Key: logs|ListAggregateLogGroupSummaries|2xx->2xx|$.aggregateLogGroupSummaries[*]
Issue-Key: logs|ListAggregateLogGroupSummaries|2xx->2xx|$.aggregateLogGroupSummaries[*].logGroupCount
Issue-Key: logs|ListAggregateLogGroupSummaries|2xx->2xx|$.nextToken
Issue-Key: logs|ListAggregateLogGroupSummaries|4xx->4xx|$.message
Issue-Commit: 02f6d4f25c27220a14c0924662b79788aefc3700

# logs export, query and log-stream refusals diverge in class, text and content (8 causes)

> _Note: this issue was identified against the code base at commit `02f6d4f25c27220a14c0924662b79788aefc3700`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateExportTask

## Description
Eight root causes on the export-task, Insights-query, query-definition, log-stream/log-event read and aggregate-log-group-summary surfaces. One call AWS refuses is accepted, one read never converges, and six refusals carry emulator-authored text. Cause count: 8 — all eight must be closed. Related open issues cover different causes with different replay keys: 20261025T181311Z-logs-s3-cloudwatch-logs-export-tasks-never-run.md and 20261026T102156Z-logs-cloudwatch-logs-export-tasks-complete-but-write-no.md (export execution), 20261029T064523Z-logs-startquery-s-inverted-time-window-refusal-carries.md (StartQuery time window), and cause 4 of 20261029T182816Z-logs-ten-cloudwatch-logs-refusal-messages-are-emulator.md (anchored patterns, which does not include the logGroupNamePattern {3,512} alternative on this wire step).

=== Cause 1 of 8: CreateExportTask accepts a destination bucket that does not exist ===
AWS: refuses with HTTP 400 InvalidParameterException and the fixed text 'The given bucket does not exist. Please make sure the bucket is valid.' (no variable parts). Smithy: CreateExportTask.destination is required (ExportDestinationBucket) and the op binds InvalidParameterException (httpError 400).
Emulator: answers HTTP 200 and returns a taskId.
Why: a client gets an export task id for an export that can never run; it then polls DescribeExportTasks forever (or reads a COMPLETED status) for data that was never written anywhere.
Fix: the destination-bucket resolution check in the CreateExportTask handler (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/).
Traces: logs-export-task-lifecycle#task#5.

=== Cause 2 of 8: StartQuery's invalid-query-string refusal is emulator-authored ===
AWS: refuses an unparseable query with HTTP 400 and a message that describes the parse failure in the query language's own terms (the recording pins the status and code; the text is the service's parser diagnostic, not a fixed sentence).
Emulator: 'The query string is not valid.' — a single generic sentence that carries no diagnostic.
Why: a user cannot tell WHERE the query is wrong, which is the entire value of the refusal; and the message does not match AWS's for any class.
Fix: the query-parse error path of the StartQuery handler — surface the parser's diagnostic.
Traces: logs-insights-start-query-and-describe-workflow#task#4 (also logs-log-group-query-discovery#task#6).

=== Cause 3 of 8: StopQuery's invalid-queryId refusal drops the id ===
AWS: 'The provided queryId=<queryId> is invalid.' — fixed text plus one variable part, the queryId exactly as supplied.
Emulator: 'The specified query does not exist.'
Why: a client stopping several queries cannot tell which id was rejected, and AWS's 'is invalid' (rather than 'does not exist') distinction is lost.
Fix: the queryId branch of the StopQuery handler.
Traces: logs-logs-insights-query-workflow#task#19.

=== Cause 4 of 8: DeleteQueryDefinition's not-found refusal is emulator-authored ===
AWS: ResourceNotFoundException with 'Query definition id is invalid. (Service: AWSLogs; Status Code: 400; Error Code: ResourceNotFoundException; Request ID: <requestId>; Proxy: null)' — fixed text including the service-suffix block, whose only per-run part is the request id.
Emulator: 'The specified query definition does not exist.'
Why: clients that match AWS's 'Query definition id is invalid.' prefix do not recognise the failure.
Fix: the not-found branch of the DeleteQueryDefinition handler.
Traces: logs-logs-insights-query-workflow#task#20.

=== Cause 5 of 8: DescribeLogStreams' orderBy/prefix conflict refusal is emulator-authored ===
AWS: refuses the combination of orderBy=LastEventTime with logStreamNamePrefix using the fixed text 'Cannot order by LastEventTime with a logStreamNamePrefix.'
Emulator: 'logStreamNamePrefix cannot be specified when orderBy is LastEventTime' — the same rule, AWS's wording discarded.
Why: status and code are right but message-matching clients mis-handle a very common refusal (two fixtures).
Fix: the orderBy/prefix validation branch of the DescribeLogStreams handler.
Traces: logsrd-describe-log-streams-golden#task#7 (also logsval-describe-log-streams-order-by-last-event-time-with-prefix#task#5).

=== Cause 6 of 8: GetLogEvents' invalid-nextToken refusal is emulator-authored ===
AWS: 'The specified nextToken is invalid.' — fixed text, no variable parts.
Emulator: 'nextToken is not a valid token'.
Why: paginating readers that key on AWS's sentence to restart from the head of the stream instead surface an unrecognised error.
Fix: the pagination-token decoder's message on the GetLogEvents path.
Traces: logsrd-get-log-events-out-of-window-read#task#7.

=== Cause 7 of 8: ListAggregateLogGroupSummaries never reports the log groups it should aggregate ===
AWS: with groupBy over a data-source dimension it returns one aggregateLogGroupSummaries entry per grouping — e.g. groupingIdentifiers [{key 'dataSource.Name', value 'Unknown'}, {key 'dataSource.Type', value 'Unknown'}] with logGroupCount 1 for a log group that has no data source — plus a nextToken. Smithy: output aggregateLogGroupSummaries (AggregateLogGroupSummaries) and nextToken, paginated.
Emulator: returns aggregateLogGroupSummaries: [] and no nextToken, and keeps doing so — the fixture polled 61 times over ~60 s and the count never appeared.
Why: a client that waits for its log group to show up in the aggregate view hangs until its own timeout; aggregation is unusable, and the un-grouped ('Unknown') bucket in particular is never produced.
Fix: the aggregation projection in the ListAggregateLogGroupSummaries handler — group the account's log groups by the requested dimension, defaulting absent data-source attributes to 'Unknown', and emit the page token.
Traces: logs-aggregate-log-group-summaries-by-datasource#task#4 (also logs-aggregate-log-group-summaries-with-name-pattern#task#4).

=== Cause 8 of 8: ListAggregateLogGroupSummaries' logGroupNamePattern refusal states a different rule and the anchored regex ===
AWS: refuses an invalid pattern with the standard constraint message "1 validation error detected: Value at 'logGroupNamePattern' failed to satisfy constraint: Member must satisfy regular expression pattern: (\\^?[\\.\\-_\\/#A-Za-z0-9]{3,512})|(\\^?[\\.\\-_\\/#A-Za-z0-9]{3,24})(\\|\\^?[\\.\\-_\\/#A-Za-z0-9]{3,24}){0,4}" — fixed template, variable parts the member name and the pattern, rendered UNANCHORED and including the {3,512} single-pattern alternative. Smithy: LogGroupNameRegexPattern @pattern '^(\\^?[\\.\\-_\\/#A-Za-z0-9]{3,24})(\\|\\^?[...]{3,24}){0,4}$' — note the model omits the {3,512} alternative the service accepts, so the recording is the wire truth.
Emulator: "1 validation error detected: Value at 'logGroupNamePattern' failed to satisfy constraint: Member must specify at most 5 '|'-separated patterns" — it replaces the regex with a prose rule of its own.
Why: a user cannot recover the accepted pattern grammar from the message, and the stated rule is narrower than what AWS accepts (a single pattern of up to 512 characters).
Fix: the logGroupNamePattern validation in the ListAggregateLogGroupSummaries handler — use the shared constraint renderer with the service's unanchored pattern.
Traces: logs-list-aggregate-log-group-summaries-invalid-pattern#task#1.

## Evidence
Cause 1: "status: expected 400, got 200; $.__type: missing (expected 'InvalidParameterException', got None); $.message: missing (expected 'The given bucket does not exist. Please make sure the bucket is valid.', got None)".
Cause 2: "$.message: value_mismatch (expected '<shaped-not-asserted>', got 'The query string is not valid.')" at HTTP 400 -> 400 — the recording pins status/code and that the message is the service's parser diagnostic, 2 affected steps.
Cause 3: "$.message: value_mismatch (expected 'The provided queryId=00000000-0000-0000-0000-000000000000 is invalid.', got 'The specified query does not exist.')" — the queryId is the request-derived variable part.
Cause 4: "$.message: value_mismatch (expected 'Query definition id is invalid. (Service: AWSLogs; Status Code: 400; Error Code: ResourceNotFoundException; Request ID: <requestId>; Proxy: null)', got 'The specified query definition does not exist.')".
Cause 5: "$.message: value_mismatch (expected 'Cannot order by LastEventTime with a logStreamNamePrefix.', got 'logStreamNamePrefix cannot be specified when orderBy is LastEventTime')" — 2 affected steps.
Cause 6: "$.message: value_mismatch (expected 'The specified nextToken is invalid.', got 'nextToken is not a valid token')".
Cause 7: "$: poll capped at 60s (expected {'aggregateLogGroupSummaries': [{'groupingIdentifiers': [{'key': 'dataSource.Name', 'value': 'Unknown'}, {'key': 'dataSource.Type', 'value': 'Unknown'}], 'logGroupCount': 1}], 'nextToken': '<shaped-not-asserted>'}, got {'aggregateLogGroupSummaries': []}); $.aggregateLogGroupSummaries[0].logGroupCount: poll: not in [1] after 61 attempt(s) (expected 1, got None); $.nextToken: missing" — 2 affected steps; the emulator served HTTP 200 throughout, so this is its own empty aggregation, not a runner failure.
Cause 8: "$.message: value_mismatch (expected \"1 validation error detected: Value at 'logGroupNamePattern' failed to satisfy constraint: Member must satisfy regular expression pattern: (\\^?[\\.\\-_\\/#A-Za-z0-9]{3,512})|(\\^?[...]{3,24})(\\|\\^?[...]{3,24}){0,4}\", got \"... Member must specify at most 5 '|'-separated patterns\")".

## Steps to Reproduce
1: CreateLogGroup + PutLogEvents, then CreateExportTask(destination='<a bucket that does not exist>', from=0, to=<now>) — expect 400 InvalidParameterException 'The given bucket does not exist. Please make sure the bucket is valid.' (logs-export-task-lifecycle#task#5).
2: StartQuery with a syntactically invalid queryString — expect 400 with the service's parser diagnostic, not a single generic sentence (logs-insights-start-query-and-describe-workflow#task#4).
3: StopQuery(queryId='00000000-0000-0000-0000-000000000000') — expect 'The provided queryId=<id> is invalid.' (logs-logs-insights-query-workflow#task#19).
4: DeleteQueryDefinition(queryDefinitionId=<unknown>) — expect 'Query definition id is invalid. (Service: AWSLogs; ...)' (#task#20).
5: DescribeLogStreams(logGroupName=..., orderBy='LastEventTime', logStreamNamePrefix='x') — expect 'Cannot order by LastEventTime with a logStreamNamePrefix.' (logsrd-describe-log-streams-golden#task#7).
6: GetLogEvents(..., nextToken='<garbage>') — expect 'The specified nextToken is invalid.' (logsrd-get-log-events-out-of-window-read#task#7).
7: create a log group with no data source, then ListAggregateLogGroupSummaries(groupBy=<the data-source dimension>) — expect one summary with groupingIdentifiers dataSource.Name/dataSource.Type = 'Unknown', logGroupCount 1, and a nextToken (logs-aggregate-log-group-summaries-by-datasource#task#4).
8: ListAggregateLogGroupSummaries(logGroupNamePattern=<an invalid pattern>) — expect the standard constraint message carrying the unanchored regex with the {3,512} alternative (logs-list-aggregate-log-group-summaries-invalid-pattern#task#1).
