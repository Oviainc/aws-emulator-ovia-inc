Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: networkflowmonitor.query-monitor-top-contributors-lifecycle-terminal networkflowmonitor.query-workload-insights-top-contributors-lifecycle-terminal networkflowmonitor.workload-insights-data-query-lifecycle networkflowmonitor.stop-query-succeeds-and-reaches-terminal networkflowmonitor.stop-query-on-completed-rejected networkflowmonitor.start-query-invalid-time-range-rejected
Issue-Shape-Id: com.amazonaws.networkflowmonitor#NetworkFlowMonitor
Issue-Key: networkflowmonitor|StartQueryMonitorTopContributors|ValidationException
Issue-Key: networkflowmonitor|StartQueryWorkloadInsightsTopContributors|ValidationException
Issue-Key: networkflowmonitor|StartQueryWorkloadInsightsTopContributorsData|ValidationException
Issue-Key: networkflowmonitor.start-query-invalid-time-range-rejected|run#12|AssertionError|assert error["Message"] == "EndTime cannot be before StartTime", (
Issue-Commit: bd3308b0c8d992d57414e8fe0564c68280e702c4

# NetworkFlowMonitor query plane unusable: date-time timestamps rejected by the request deserializer, no time-range validation, and every StartQuery/StopQuery/GetQueryResults handler is a 501 stub (3 causes)

> _Note: this issue was identified against the code base at commit `bd3308b0c8d992d57414e8fe0564c68280e702c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryMonitorTopContributors

## Description
Three distinct root causes make the whole NetworkFlowMonitor query plane (monitor top-contributors and workload-insights queries) unreachable for a normal SDK client. All three are in service networkflowmonitor.

=== Cause 1 of 3: request deserializer rejects modeled `date-time` timestamps ===
AWS: Smithy model services/service-com-amazonaws-networkflowmonitor-networkflowmonitor/smithy-model/com-amazonaws-networkflowmonitor-networkflowmonitor.json — StartQueryMonitorTopContributors.startTime and .endTime target `Timestamp` with `timestampFormat: date-time` and are `@required`; the same members with the same trait appear on StartQueryWorkloadInsightsTopContributors and StartQueryWorkloadInsightsTopContributorsData. The protocol is restJson1, so an AWS SDK puts these on the wire as RFC-3339 / ISO-8601 strings (e.g. "2026-10-22T20:07:07Z"), and real AWS accepts them and returns 200 with a `queryId`.
Emulator: every call made by a stock botocore client with `datetime` values is refused before the handler runs with HTTP 400 `ValidationException`, message example `invalid timestamp: failed to parse int`. The message shows the deserializer is reading the member as an epoch integer (the Smithy default `epoch-seconds`) and ignoring the member's `@timestampFormat("date-time")` override, so the ISO-8601 string can never parse.
Why: a client cannot start ANY query. Because the refusal happens during deserialization, it precedes the handler, so no query is ever created, no `queryId` is returned, and the rest of the query lifecycle (StopQuery*, GetQueryResults*) is unreachable. A client that sends exactly what the model specifies is told its own valid input is invalid.
Fix: the service's restJson1 request-deserialization path for `Timestamp` members — it must honour the member-level `@timestampFormat("date-time")` carried in the generated input schemas (START_QUERY_MONITOR_TOP_CONTRIBUTORS_INPUT_SCHEMA and the two workload-insights equivalents in services/service-com-amazonaws-networkflowmonitor-networkflowmonitor/src/ops/types.rs) rather than parsing an integer.
Traces: networkflowmonitor.query-monitor-top-contributors-lifecycle-terminal, networkflowmonitor.query-workload-insights-top-contributors-lifecycle-terminal, networkflowmonitor.workload-insights-data-query-lifecycle, networkflowmonitor.stop-query-succeeds-and-reaches-terminal, networkflowmonitor.stop-query-on-completed-rejected, networkflowmonitor.start-query-invalid-time-range-rejected.

=== Cause 2 of 3: startTime-after-endTime is not validated as a time range ===
AWS: StartQueryMonitorTopContributors models `ValidationException` (HTTP 400) and refuses a request whose startTime is later than its endTime. Recorded behavior: HTTP 400, code `ValidationException`, message `EndTime cannot be before StartTime` (a fixed, request-independent text for this condition). No queryId is produced.
Emulator: the call is refused with code `ValidationException` (correct) but for the wrong reason — the message is `invalid timestamp: failed to parse int`, i.e. the request died in Cause 1's timestamp parse and the range relation was never evaluated at all. There is therefore no evidence any range check exists; once Cause 1 is fixed this request would reach a handler that performs no start/end ordering check.
Why: a client relying on AWS to reject an inverted window gets no signal about the real problem, and after Cause 1 is fixed would get an accepted query over an impossible window instead of a 400.
Fix: input validation for the three StartQuery* operations — after timestamps deserialize, compare startTime and endTime and return the modeled `ValidationException` (HTTP 400) when endTime precedes startTime.
Traces: networkflowmonitor.start-query-invalid-time-range-rejected.

=== Cause 3 of 3: the entire query plane is still scaffolded, so it answers 501 NotImplemented ===
AWS: the model binds StartQueryMonitorTopContributors (POST /monitors/{monitorName}/topContributorsQueries), StartQueryWorkloadInsightsTopContributors (POST /workloadInsights/{scopeId}/topContributorsQueries), StartQueryWorkloadInsightsTopContributorsData (POST /workloadInsights/{scopeId}/topContributorsDataQueries), each returning 200 with a required `queryId`, plus the matching StopQuery*/GetQueryResults* operations. None of them models a 501, and `NotImplemented` is not a member of any of their error sets (AccessDeniedException 403, InternalServerException 500, ServiceQuotaExceededException 402, ThrottlingException 429, ValidationException 400).
Emulator: services/service-com-amazonaws-networkflowmonitor-networkflowmonitor/src/ops/start_query_monitor_top_contributors.rs still carries the `SCAFFOLDED, NOT IMPLEMENTED` marker and its `handle` body returns `Err(crate::scaffolded::Scaffolded::not_implemented("StartQueryMonitorTopContributors"))` → HTTP 501 `NotImplemented`. The sibling query operations are in the same state. This cause is masked today by Cause 1 (the 400 fires first), so fixing only Cause 1 converts these checks from one wrong answer to another.
Why: a client cannot start, stop, poll, or read results for any query; an unmodeled 501/`NotImplemented` is also not a code any SDK error handler can classify.
Fix: implement the query lifecycle in the query operations' own handler files (src/ops/start_query_monitor_top_contributors.rs, src/ops/start_query_workload_insights_top_contributors.rs, src/ops/start_query_workload_insights_top_contributors_data.rs and the corresponding stop/get-results handlers), backed by query state in src/state.rs, and delete the SCAFFOLDED markers.
Traces: networkflowmonitor.query-monitor-top-contributors-lifecycle-terminal, networkflowmonitor.query-workload-insights-top-contributors-lifecycle-terminal, networkflowmonitor.workload-insights-data-query-lifecycle, networkflowmonitor.stop-query-succeeds-and-reaches-terminal, networkflowmonitor.stop-query-on-completed-rejected.

Note on scope: no currently open issue covers these. The auto-filed scaffold issues for this service name ListScopes, DeleteScope, ListTagsForResource and UpdateMonitor only; none of the three StartQuery* operations, and no issue covers the timestamp deserialization defect (which is a pre-handler failure, not a stub call).

## Evidence
Suite: 4/24 passed for networkflowmonitor.

Cause 1 — six checks died on a StartQuery* call made with stock botocore datetime arguments:
  networkflowmonitor.query-monitor-top-contributors-lifecycle-terminal:
    botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the StartQueryMonitorTopContributors operation: invalid timestamp: failed to parse int
  networkflowmonitor.query-workload-insights-top-contributors-lifecycle-terminal:
    ... (ValidationException) when calling the StartQueryWorkloadInsightsTopContributors operation: invalid timestamp: failed to parse int
  networkflowmonitor.workload-insights-data-query-lifecycle:
    ... (ValidationException) when calling the StartQueryWorkloadInsightsTopContributorsData operation: invalid timestamp: failed to parse int
  networkflowmonitor.stop-query-succeeds-and-reaches-terminal and networkflowmonitor.stop-query-on-completed-rejected:
    ... (ValidationException) when calling the StartQueryMonitorTopContributors operation: invalid timestamp: failed to parse int
  Expected: HTTP 200 with a `queryId` (startTime/endTime are `@timestampFormat("date-time")`, so the wire value is an ISO-8601 string).
  Actual: HTTP 400 ValidationException claiming an int parse failure.

Cause 2 — networkflowmonitor.start-query-invalid-time-range-rejected sends startTime=now, endTime=now-1h:
  assert error["Message"] == "EndTime cannot be before StartTime"
  AssertionError: expected the exact refusal message for startTime after endTime, got 'invalid timestamp: failed to parse int'
  Expected: HTTP 400 ValidationException, message `EndTime cannot be before StartTime`.
  Actual: HTTP 400 ValidationException from the timestamp parser; the range was never compared.

Cause 3 — from the checkout: services/service-com-amazonaws-networkflowmonitor-networkflowmonitor/src/ops/start_query_monitor_top_contributors.rs lines 3-6 carry `//! SCAFFOLDED, NOT IMPLEMENTED`, and line 188 of the same file is `Err(crate::scaffolded::Scaffolded::not_implemented("StartQueryMonitorTopContributors"))`, i.e. HTTP 501 `NotImplemented` — a status/code none of the operation's modeled errors (403/500/402/429/400) allows.

## Steps to Reproduce
1. Boot the emulator and point a botocore/boto3 `networkflowmonitor` client at it.
2. Create a scope (CreateScope) and a monitor (CreateMonitor) in the caller's account/Region; wait for the monitor to go ACTIVE.
3. Cause 1: call `start_query_monitor_top_contributors(monitorName=<name>, startTime=<aware datetime now-1h>, endTime=<aware datetime now>, metricName="DATA_TRANSFERRED", destinationCategory="UNCLASSIFIED")`. Observe HTTP 400 `ValidationException: invalid timestamp: failed to parse int` instead of HTTP 200 with a `queryId`. Repeat against `start_query_workload_insights_top_contributors(scopeId=...)` and `start_query_workload_insights_top_contributors_data(scopeId=...)` for the same result.
4. Cause 2: repeat step 3 with `startTime=now` and `endTime=now-1h`. Expect HTTP 400 `ValidationException` with message `EndTime cannot be before StartTime`; observe the int-parse message instead.
5. Cause 3: send the same POST with an epoch-integer timestamp (bypassing Cause 1) to `/monitors/{monitorName}/topContributorsQueries`. Observe HTTP 501 `NotImplemented`, and likewise for StopQuery*/GetQueryResults*.
6. Re-replay: networkflowmonitor.query-monitor-top-contributors-lifecycle-terminal, networkflowmonitor.query-workload-insights-top-contributors-lifecycle-terminal, networkflowmonitor.workload-insights-data-query-lifecycle, networkflowmonitor.stop-query-succeeds-and-reaches-terminal, networkflowmonitor.stop-query-on-completed-rejected, networkflowmonitor.start-query-invalid-time-range-rejected.
