Issue-Source: trace-sweep-divergence
Issue-Trace: logs-insights-query-single-log-group
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-insights-query-single-log-group logs-insights-query-workflow logs-describe-queries-pagination logs-log-group-lifecycle-and-tags logs-logs-insights-query-workflow logs-metric-filter-alerting-pipeline logs-metric-filter-configure-test-and-verify logs-log-anomaly-detector-lifecycle logs-get-log-fields-for-managed-data-sources logs-get-log-fields-unknown-data-source logs-log-fields-generic-data-source logs-logfields-and-loggroupfields-and-querygroups
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|DescribeQueries|2xx->2xx|$.queries[*]
Issue-Key: logs|DescribeQueries|2xx->2xx|$.queries[*].bytesScanned
Issue-Key: logs|DescribeQueries|2xx->2xx|$.queries[*].queryDuration
Issue-Key: logs|DescribeQueries|2xx->2xx|$.queries[*].queryId
Issue-Key: logs|DescribeQueries|2xx->2xx|$.queries[*].queryString
Issue-Key: logs|DescribeQueries|2xx->2xx|$.queries[*].userIdentity
Issue-Key: logs|DescribeLogGroups|2xx->2xx|$.logGroups[arn="arn
Issue-Key: logs|ListLogGroups|2xx->2xx|$.nextToken
Issue-Key: logs|TestMetricFilter|2xx->2xx|$.matches[*].extractedValues
Issue-Key: logs|GetLogAnomalyDetector|2xx->2xx|$.anomalyVisibilityTime
Issue-Key: logs|GetLogFields|2xx->2xx|$.logFields[*]
Issue-Commit: 02f6d4f25c27220a14c0924662b79788aefc3700

# logs read paths omit modeled response members and serve non-canonical values (9 causes)

> _Note: this issue was identified against the code base at commit `02f6d4f25c27220a14c0924662b79788aefc3700`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeQueries

## Description
Nine root causes on CloudWatch Logs read paths. Every one replays at HTTP 200 -> 200: the status and error contract are correct and only the response body diverges — either a member AWS always populates is absent, or a value the emulator stores verbatim is served where AWS serves a canonicalized/derived one. Cause count: 9 — all nine must be closed.

=== Cause 1 of 9: DescribeQueries omits bytesScanned, queryDuration and userIdentity, and does not canonicalize queryString ===
AWS: each QueryInfo carries bytesScanned (double, 0.0 for a query that scanned nothing), queryDuration (long, ms) and userIdentity (the ARN of the user who ran the query) in addition to queryId/queryString/status/createTime/logGroupName. Smithy: QueryInfo.bytesScanned (double, 'total number of bytes scanned'), queryDuration (long, 'duration in milliseconds'), userIdentity ('ARN of the user who ran the query'). AWS also stores the EFFECTIVE query string: a StartQuery with logGroupName plus startTime/endTime is rewritten to the SOURCE-prefixed CWLI form 'SOURCE "<logGroupName>" START=<startTime> END=<endTime> | <submitted query>' and the trailing '| limit <n>' the request's `limit` implies is appended.
Emulator: the three members are absent and queryString is the raw submitted string.
Why: cost/latency dashboards built on bytesScanned+queryDuration see nulls, audit tooling loses the caller ARN, and a client that correlates the stored query string with what it submitted (or re-submits it) gets a string AWS would not have returned.
Fix: the QueryInfo projection in the DescribeQueries handler plus the query-string canonicalization in StartQuery's state record (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/).
Traces: logs-insights-query-single-log-group#task#6 (also logs-insights-query-workflow#task#3).

=== Cause 2 of 9: DescribeQueries pagination serves the wrong page's query ===
AWS: with maxResults/nextToken the page's first QueryInfo is the next query in AWS's ordering — the recorded page's queryId is the third query created, not the first — and its queryString is the canonical SOURCE-prefixed form of THAT query (including its own '| limit 12').
Emulator: returns the first query again (same queryId as page 1) with the raw query string and no bytesScanned/queryDuration/userIdentity.
Why: a paginating client re-reads one query forever and never sees the rest.
Fix: the DescribeQueries pagination cursor (order and offset) in the same handler.
Traces: logs-describe-queries-pagination#task#8 (also #task#10).

=== Cause 3 of 9: DescribeQueries omits the same three members on the remaining pages ===
AWS: bytesScanned 0.0, queryDuration and userIdentity are present on every page, and queryString is canonicalized there too.
Emulator: absent / raw.
Why: same as cause 1, on the paginated read.
Fix: same projection (this cluster is a distinct page/step, so it must be re-replayed separately).
Traces: logs-describe-queries-pagination#task#9.

=== Cause 4 of 9: DescribeLogGroups reports a non-zero storedBytes for a freshly written group ===
AWS: storedBytes for a log group whose events have not yet been rolled up is 0 — the value is a metered aggregate that AWS does not update synchronously with PutLogEvents.
Emulator: reports the byte count of the ingested events immediately.
Why: a client that asserts a new group reports 0 stored bytes (or that computes cost from this field right after ingest) gets a value AWS never shows at that point. Only the zero-versus-nonzero contract is the requirement; no particular byte count is.
Fix: the storedBytes projection in the DescribeLogGroups handler / log-group state.
Traces: logs-insights-query-single-log-group#task#5.

=== Cause 5 of 9: ListLogGroups omits nextToken ===
AWS: returns a nextToken when more log groups remain beyond the returned page.
Emulator: the member is absent, so the page looks terminal.
Why: a client enumerating log groups stops after the first page and silently misses the rest.
Fix: the pagination cursor emitted by the ListLogGroups handler.
Traces: logs-log-group-lifecycle-and-tags#task#3.

=== Cause 6 of 9: TestMetricFilter omits extractedValues on a match ===
AWS: every MetricFilterMatchRecord carries extractedValues — an EMPTY map when the filter pattern extracts no named values. Smithy: MetricFilterMatchRecord.extractedValues (map, 'values extracted from the event data by the filter').
Emulator: the member is absent.
Why: a client that iterates match.extractedValues to preview what a metric filter would emit dereferences null instead of an empty map.
Fix: the TestMetricFilter serializer.
Traces: logs-logs-insights-query-workflow#task#4 (also #task#5, logs-metric-filter-alerting-pipeline#task#2, logs-metric-filter-configure-test-and-verify#task#4).

=== Cause 7 of 9: GetLogAnomalyDetector omits anomalyVisibilityTime ===
AWS: returns anomalyVisibilityTime — the detector's effective value, defaulted to 7 days when CreateLogAnomalyDetector omitted it. Smithy: GetLogAnomalyDetector output anomalyVisibilityTime (@range 7..90).
Emulator: the member is absent.
Why: a client reading back the detector cannot learn its visibility window, so an update that preserves it is impossible.
Fix: default anomalyVisibilityTime at create time and serialize it in GetLogAnomalyDetector.
Traces: logs-log-anomaly-detector-lifecycle#task#10.

=== Cause 8 of 9: GetLogFields returns the wrong field order and the wrong types for a managed data source ===
AWS: the logFields list is ordered with @timestamp first and each entry's logFieldType.type is the semantic type — @timestamp and @ingestionTime are 'timestamp'.
Emulator: orders @ingestionTime first and types the time fields as 'integer'.
Why: a client that renders the field list, or that picks a time field by type to build a query, gets the wrong field and the wrong type.
Fix: the GetLogFields field catalog (order and LogFieldType.type) in its handler.
Traces: logs-get-log-fields-for-managed-data-sources#task#1 (also #task#2).

=== Cause 9 of 9: GetLogFields omits seven fields for a generic/unknown data source ===
AWS: returns, in order, @timestamp, ..., @ingestionTime, ..., @logStreamId plus @aws.account, @aws.region, @source.log, @data_format, @data_source_name, @data_source_type and @logGroupId (all 'string').
Emulator: returns only the five @-prefixed Insights fields, in the wrong order and with 'integer' time types; the seven data-source fields are absent entirely.
Why: a client discovering queryable fields for an arbitrary data source cannot see the account/region/source/data-source dimensions, so queries it builds reference fields it believes do not exist.
Fix: the same field catalog — add the generic data-source field set.
Traces: logs-get-log-fields-unknown-data-source#task#1 (also logs-log-fields-generic-data-source#task#1, logs-logfields-and-loggroupfields-and-querygroups#task#4).

## Evidence
Cause 1: "$.queries[0]: missing (expected {'bytesScanned': '<shaped-not-asserted>', 'createTime': '<shaped-not-asserted>', 'logGroupName': <group>, 'queryDuration': '<shaped-not-asserted>', 'queryId': ..., 'queryLanguage': 'CWLI', 'queryString': 'SOURCE \"<group>\" START=0 END=4102444800999 | fields @timestamp, @message | sort @timestamp desc | limit 20 | limit 20', 'status': 'Complete', 'userIdentity': '<shaped-not-asserted>'}, got {'queryLanguage': 'CWLI', 'queryId': ..., 'queryString': 'fields @timestamp, @message | sort @timestamp desc | limit 20', 'status': 'Complete', 'createTime': ..., 'logGroupName': <group>})" — createTime/queryId/userIdentity are per-run values; the SOURCE-prefix rewrite and the three present-but-absent members are the requirement.
Cause 2: "$.queries[0].queryId: value_mismatch (expected '...-000000000003', got '...-000000000001'); $.queries[0].queryString: value_mismatch (expected 'SOURCE \"<group>\" START=0 END=4102444800999 | fields @message | filter @message like /level/ | limit 12', got 'fields @message | filter @message like /level/ | limit 10'); $.queries[0].bytesScanned: missing (expected 0.0, got None); $.queries[0].queryDuration / userIdentity: missing".
Cause 3: "$.queries[0].bytesScanned: missing (expected 0.0, got None); $.queries[0].queryDuration: missing; $.queries[0].queryString: value_mismatch (expected 'SOURCE \"<group>\" START=0 END=4102444800999 | ... | limit 11', got 'fields @message | filter @message like /level/ | limit 11'); $.queries[0].userIdentity: missing".
Cause 4: "$.logGroups[arn=...].storedBytes: value_mismatch (expected 0, got 270)".
Cause 5: "$.nextToken: missing (expected '<shaped-not-asserted>', got None)".
Cause 6: "$.matches[0].extractedValues: missing (expected {}, got None)" — 4 affected steps.
Cause 7: "$.anomalyVisibilityTime: missing (expected 7, got None)".
Cause 8: "$.logFields[0]: missing (expected {'logFieldName': '@timestamp', 'logFieldType': {'type': 'timestamp'}}, got {'logFieldName': '@ingestionTime', 'logFieldType': {'type': 'integer'}}); $.logFields[2]: missing (expected {'logFieldName': '@ingestionTime', 'logFieldType': {'type': 'timestamp'}}, got {'logFieldName': '@logStream', ...})".
Cause 9: the same two mismatches plus "$.logFields[4]: missing (expected {'logFieldName': '@logStreamId', ...}, got {'logFieldName': '@timestamp', 'logFieldType': {'type': 'integer'}}); $.logFields[6..12]: missing (expected @aws.account, @aws.region, @source.log, @data_format, @data_source_name, @data_source_type, @logGroupId — all {'type': 'string'} — got None)".

## Steps to Reproduce
1/2/3: CreateLogGroup, PutLogEvents, StartQuery(logGroupName=..., startTime=0, endTime=4102444800999, queryString='fields @timestamp, @message | sort @timestamp desc | limit 20', limit=20), then DescribeQueries — assert bytesScanned/queryDuration/userIdentity are present and queryString is the SOURCE-prefixed canonical form; repeat with maxResults=1 and follow nextToken and assert each page returns the NEXT query (logs-insights-query-single-log-group#task#6, logs-describe-queries-pagination#task#8, #task#9).
4: CreateLogGroup, PutLogEvents, DescribeLogGroups — assert storedBytes == 0 (logs-insights-query-single-log-group#task#5).
5: create more log groups than one page holds and call ListLogGroups — assert nextToken is present (logs-log-group-lifecycle-and-tags#task#3).
6: TestMetricFilter with a pattern that matches but extracts no named values — assert matches[0].extractedValues == {} (logs-logs-insights-query-workflow#task#4).
7: CreateLogAnomalyDetector without anomalyVisibilityTime, then GetLogAnomalyDetector — assert anomalyVisibilityTime == 7 (logs-log-anomaly-detector-lifecycle#task#10).
8/9: GetLogFields for a managed data source and for an unknown/generic one — assert the field order, the 'timestamp' types for @timestamp/@ingestionTime, and the presence of @logStreamId, @aws.account, @aws.region, @source.log, @data_format, @data_source_name, @data_source_type, @logGroupId (logs-get-log-fields-for-managed-data-sources#task#1, logs-get-log-fields-unknown-data-source#task#1).
