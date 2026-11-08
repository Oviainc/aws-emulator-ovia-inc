Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.logs-insights-semantics-syntax-error-rejected-at-start logs.list-validation-error-semantics
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.logs-insights-semantics-syntax-error-rejected-at-start|_start_error#17|AssertionError|raise AssertionError(f"StartQuery accepted a query real AWS refuses: {kwargs.get('queryString')!r} -> {resp!r}")
Issue-Key: logs.list-validation-error-semantics|run#14|AssertionError|assert code_query == "AccessDeniedException", (
Issue-Commit: 5fa99114f4f1697b9b790323162b082bd594102a

# logs: StartQuery accepts a query with a dangling trailing command, and ListLogGroupsForQuery answers 501 NotImplemented (2 causes)

> _Note: this issue was identified against the code base at commit `5fa99114f4f1697b9b790323162b082bd594102a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQuery

## Description
Two independent, net-new root causes in the CloudWatch Logs emulator. Cause count: 2. Each cause below has its own failing check ids; BOTH must be closed before submitting.

=== Cause 1 of 2: StartQuery accepts a Logs Insights query whose trailing pipe command has no operand ===
AWS:      Smithy (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/smithy-model/com-amazonaws-cloudwatchlogs-logs-20140328.json) models StartQuery with `queryString` @required (length 0..10000) and MalformedQueryException among its errors (errorKind client, httpError 400). The model does not describe the query grammar, so for the grammar rule: recorded behavior only. The recording shows StartQuery parses the whole query BEFORE minting a queryId and refuses a syntactically incomplete query synchronously with HTTP 400 / MalformedQueryException, no queryId, and nothing left behind (DescribeQueries for the log group never lists the refused query string). For a query that ends in a command with no operand (`fields @message | filter @message like '<marker>' | sort`) the refusal message is the fixed text `unexpected symbol found <EOF> at line 1 and position ` followed by one variable part: the 1-based character position at which the parser hit end-of-input. (AWS appends its own request-id suffix to the sentence, which the response does not expose; the position is absolute within the statement AWS parses, which is the caller's string prefixed by a service-rewritten SOURCE clause, so the requirement on the number is that it TRACKS the offending token: between two queries that differ only by the offset of the bad token, the reported positions must differ by exactly that offset.) The same operation already refuses an unknown command correctly in this emulator (`... | frobnicate x` -> MalformedQueryException `unexpected symbol found frobnicate at line 1 and position <n>`), so only the end-of-input branch is missing.
Emulator: the dangling-command query is ACCEPTED with HTTP 200 and a minted queryId (`{'queryId': '<uuid>', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}`), i.e. no MalformedQueryException at all.
Why:      a client that submits a truncated or programmatically-assembled query (a trailing `| sort`, `| stats`, `| filter` with the operand lost) is told the query started. It then polls GetQueryResults for a query that can never produce results instead of branching on the synchronous MalformedQueryException AWS returns, and it loses the position information it would use to point the user at the broken token. The refused query must also not become visible through DescribeQueries.
Fix:      the query-string parse/validation stage of the StartQuery handler (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/start_query.rs) — the same validator that already produces `unexpected symbol found <token> at line 1 and position <n>` for an unknown command must also report end-of-input as `<EOF>` with a position consistent with the token positions it reports for other symbols, and must store nothing. Related but NOT covering this cause: open issues 20261029T182816Z-logs-startquery-and-eight-other-logs-operations-accept.md (its cause 3 is the unknown-stats-function and SQL-with-logGroupName acceptances) and 20261025T192833Z-logs-cloudwatch-logs-7-net-new-root-causes-startquery.md — neither carries this check's replay key, and neither mentions the end-of-input branch.
Traces:   logs.logs-insights-semantics-syntax-error-rejected-at-start (the check's first assertion, the unknown-command refusal, PASSES; it fails on the dangling `| sort`).

=== Cause 2 of 2: ListLogGroupsForQuery is an unimplemented scaffold stub, so every refusal it owes is unreachable ===
AWS:      Smithy models ListLogGroupsForQuery with `queryId` @required (length 1..256), `maxResults` range 50..500, and the errors AccessDeniedException, InvalidParameterException, ResourceNotFoundException (all errorKind client, httpError 400) and ServiceUnavailableException (500). Recorded behavior for the two paths exercised: (a) a well-formed but nonexistent `queryId` is refused with error code `AccessDeniedException` and an EMPTY message (no fixed text, no variable parts — AWS sends the code with `Message` empty); (b) `maxResults=501`, above the modeled maximum of 500, is refused with `InvalidParameterException` and the message `1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 500` — fixed text whose variable parts are the member name (`maxResults`) and the limit (`500`) taken from the model. botocore does not validate the upper bound locally, so the service must enforce it.
Emulator: the operation is not implemented. services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/list_log_groups_for_query.rs `handle()` binds its arguments and returns `Err(crate::scaffolded::Scaffolded::not_implemented("ListLogGroupsForQuery"))`, so every call answers the 501-class error code `NotImplemented`. Replay: `ListLogGroupsForQuery(queryId=<nonexistent>): expected AccessDeniedException, got 'NotImplemented'`. The maxResults assertion that follows is therefore never reached either.
Why:      a client cannot enumerate the log groups a query ran against at all, and the refusal it does get is unclassifiable: `NotImplemented` is neither the authorization refusal (AccessDeniedException) nor the parameter refusal (InvalidParameterException) the SDK error-handling branches on, and no retry/permission remediation path matches it.
Fix:      implement the ListLogGroupsForQuery handler (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/list_log_groups_for_query.rs): resolve `queryId` against the query store and return the query's log-group identifiers, refuse an unknown/unauthorized queryId with AccessDeniedException carrying an empty message, and enforce the modeled maxResults upper bound with InvalidParameterException and AWS's validation sentence. No issue exists for this operation (a search of open and fixed issues for "ListLogGroupsForQuery" returns none), and the replay did not mark this check as blocked by a filed stub.
Traces:   logs.list-validation-error-semantics (the check's earlier ListAggregateLogGroupSummaries groupBy-enum and logGroupNamePattern assertions PASS; it fails on the first ListLogGroupsForQuery call).

## Evidence
Replay quotes (per-run ids, markers and generated names inside them are not part of the requirement):

Cause 1 (logs.logs-insights-semantics-syntax-error-rejected-at-start, [mismatch]):
  AssertionError: StartQuery accepted a query real AWS refuses: "fields @message | filter @message like 'mk<marker>' | sort" -> {'queryId': '00000000-0000-4000-8000-000000000001', 'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '50', ...}, 'RetryAttempts': 0}}
  Expected: HTTP 400, Error.Code 'MalformedQueryException', Message matching `unexpected symbol found <EOF> at line 1 and position (\d+)`, no queryId minted, and the refused string absent from DescribeQueries. The preceding assertion in the same check shows the emulator DOES answer MalformedQueryException with `unexpected symbol found frobnicate at line 1 and position <n>` for an unknown command, so only the end-of-input branch diverges. Model: StartQuery.errors includes MalformedQueryException (httpError 400).

Cause 2 (logs.list-validation-error-semantics, [mismatch]):
  AssertionError: ListLogGroupsForQuery(queryId=<nonexistent>): expected AccessDeniedException, got 'NotImplemented'
  Expected: Error.Code 'AccessDeniedException' with an empty Message for a well-formed-but-nonexistent queryId; then Error.Code 'InvalidParameterException' with "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 500" for maxResults=501. Code: services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/list_log_groups_for_query.rs, handle() returns Scaffolded::not_implemented("ListLogGroupsForQuery") (the only statement in the body besides `let _ = (&input, ctx, &self.engine);`). Model: ListLogGroupsForQuery.errors = AccessDeniedException | InvalidParameterException | ResourceNotFoundException | ServiceUnavailableException; maxResults range {min: 50, max: 500}.

## Steps to Reproduce
Cause 1:
1. Boot the emulator.
2. logs.create_log_group(logGroupName=<g>); create_log_stream(logGroupName=<g>, logStreamName='s'); put_log_events one event with message 'hello <marker>'.
3. logs.start_query(logGroupNames=[<g>], startTime=<now-600>, endTime=<now+60>, queryString="fields @message | filter @message like '<marker>' | frobnicate x") -> observe the emulator correctly raises MalformedQueryException naming 'frobnicate' and a position.
4. logs.start_query(same window, queryString="fields @message | filter @message like '<marker>' | sort") -> observe HTTP 200 and a queryId. AWS refuses with MalformedQueryException 'unexpected symbol found <EOF> at line 1 and position <n>', where <n> is offset from the position reported in step 3 by exactly the number of characters the offending token moved.
5. Also confirm neither refused query string appears in logs.describe_queries(logGroupName=<g>).
6. Replay logs.logs-insights-semantics-syntax-error-rejected-at-start.

Cause 2:
1. Boot the emulator.
2. logs.list_log_groups_for_query(queryId=str(uuid.uuid4())) -> observe Error.Code 'NotImplemented' (501 class); AWS answers 'AccessDeniedException' with an empty message.
3. logs.list_log_groups_for_query(queryId=<same>, maxResults=501) -> AWS answers 'InvalidParameterException' with "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 500"; the emulator cannot reach that branch.
4. Replay logs.list-validation-error-semantics.
