Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: internetmonitor.get-query-results-schema-for-empty-monitor internetmonitor.list-internet-events-returns-valid-structure internetmonitor.start-query-invalid-time-range-rejected
Issue-Shape-Id: com.amazonaws.internetmonitor#InternetMonitor20210603
Issue-Key: internetmonitor.get-query-results-schema-for-empty-monitor|run#11|AssertionError|assert code == "ValidationException", f"expected ValidationException for bogus QueryId, got: {code}"
Issue-Key: internetmonitor.list-internet-events-returns-valid-structure|run#8|AssertionError|assert events, "ListInternetEvents returned no AVAILABILITY events in the last 24h"
Issue-Key: internetmonitor.start-query-invalid-time-range-rejected|run#6|AssertionError|assert message == "Invalid time window specified", f"unexpected message for StartTime>EndTime: {message!r}"
Issue-Commit: 42112e68a35449969ef0708992459a3b7df02e34

# internetmonitor: GetQueryResults answers 501 NotImplemented, ListInternetEvents serves an empty global outages feed, and its inverted-time-window refusal carries an emulator-authored message (3 causes)

> _Note: this issue was identified against the code base at commit `42112e68a35449969ef0708992459a3b7df02e34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueryResults, ListInternetEvents

## Description
THREE net-new root causes on the internetmonitor query/internet-event read surface. All three must be closed and re-replayed before submitting.

Model consulted with smithy_query on services/service-com-amazonaws-internetmonitor-internetmonitor20210603/smithy-model/com-amazonaws-internetmonitor-internetmonitor20210603.json.

Cause 1 of 3: GetQueryResults is still an unimplemented scaffold stub, so a QueryId the service never minted is answered 501 NotImplemented instead of the modeled 400 ValidationException.
AWS: Smithy GetQueryResults = GET /v20210603/Monitors/{MonitorName}/Queries/{QueryId}/Results, required input MonitorName (ResourceName, len 1..255, pattern ^[a-zA-Z0-9_.-]+$) and QueryId (String), output Fields (QueryFields, required) + Data (QueryData, required) + NextToken; modeled errors AccessDeniedException(403), LimitExceededException(403), ThrottlingException(429), ValidationException(400, doc "Invalid request.", single `message` member), InternalServerException(500). Recorded behavior: for a syntactically valid but never-minted QueryId on a healthy monitor (here a monitor with no monitored resources, under which StartQuery is refused for every QueryType, so no QueryId can ever legitimately exist), the call is refused with HTTP 400, error code ValidationException and message "Invalid query ID provided" — the whole message is FIXED text, it carries no value from the request or the store. A 200 with an empty-but-well-formed Fields/Data schema is NOT served for an unknown QueryId.
Emulator: HTTP 501 with error code NotImplemented — src/ops/get_query_results.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED` marker and its handle() body ends in crate::scaffolded::Scaffolded::not_implemented("GetQueryResults"), so nothing about this operation is served: neither the refusal for an unknown QueryId nor the result schema for a real one.
Why: the query interface has no readable results at all. A client gets an unmodeled 501/NotImplemented that no SDK error branch expects, so code catching ValidationException to distinguish "that query id is not mine" from a transport failure never catches it, and any retry loop treats a permanent client error as a server-side outage. Note this is a DIFFERENT operation from the sibling stubs GetQueryStatus/StopQuery already closed by the resolved issue 20261023T171609Z-internetmonitor-7-refusal-path-causes-wrong-error-shapes.md (cause 7) — that fix left GetQueryResults untouched, and no open issue names it.
Fix: implement the GetQueryResults handler in src/ops/get_query_results.rs — resolve (MonitorName, QueryId) against the query store in src/state.rs, refuse an unknown QueryId with the modeled ValidationException and AWS's fixed text, and project Fields/Data for a known one; then delete the SCAFFOLDED marker line.
Traces: internetmonitor.get-query-results-schema-for-empty-monitor.

Cause 2 of 3: ListInternetEvents serves an empty InternetEvents list — the account-independent global outages feed is never populated.
AWS: Smithy ListInternetEvents = GET /v20210603/InternetEvents, @readonly, @paginated(items: InternetEvents); output InternetEvents (InternetEventsList, required). Each InternetEventSummary requires EventId (len 1..255, pattern ^[a-zA-Z0-9-]+$), EventArn (Arn, pattern ^arn:.*), StartedAt (Timestamp, @timestampFormat date-time), ClientLocation, EventType (InternetEventType) and EventStatus (InternetEventStatus); EndedAt is empty while the event is unresolved. The operation doc states Internet Monitor "displays information about recent global health events, called internet events, on a global outages map that is available to all Amazon Web Services customers" — i.e. the feed is a property of the service, not of anything the caller created. Recorded behavior: a 24h window with EventType=AVAILABILITY returns a non-empty page of AVAILABILITY events for any account with no monitor configured (the replay recorded >80), each with EventStatus in (ACTIVE, RESOLVED) and EventArn of the form arn:aws:internetmonitor::<accountId>:internet-event/<EventId> (fixed text "arn:aws:internetmonitor::" and ":internet-event/"; variable parts: the serving account id and that event's EventId).
Emulator: HTTP 200 with InternetEvents: [] for every window and every filter. src/ops/list_internet_events.rs reports exactly what crate::state's `internet_events` BTreeMap holds, and nothing ever writes it: state.rs documents `record_internet_event` as having "No operation of this service calls it", so the store is empty for the whole process lifetime. The filtering, paging and NextToken logic is therefore never exercised either.
Why: the global outages feed is unobservable. A client that lists internet events to render the outages map, to correlate an application incident with a public-internet impairment, or to page through the feed, reads an empty result that is indistinguishable from "the internet was healthy" — and every downstream assertion about event shape (EventArn construction, EventStatus domain, date-time StartedAt) is silently unreachable. GetInternetEvent for any id is likewise unreachable.
Fix: the internet-event store in src/state.rs — seed the service's own observations at engine construction (the writer `record_internet_event` already exists and is the single source of truth) so the @readonly reporting operations ListInternetEvents/GetInternetEvent answer from a populated feed spanning a recent window, with EventType/EventStatus drawn from the modeled enums and EventArn built by `internet_event_arn`.
Traces: internetmonitor.list-internet-events-returns-valid-structure.

Cause 3 of 3: the inverted-time-window refusal on ListInternetEvents carries an emulator-authored message instead of AWS's wording.
AWS: Smithy ListInternetEvents takes StartTime/EndTime as @httpQuery Timestamps (@timestampFormat date-time) and models ValidationException at HTTP 400. Recorded behavior: StartTime after EndTime is refused with HTTP 400, error code ValidationException and message "Invalid time window specified" — the entire message is FIXED text and names neither bound, neither member nor any value from the request; the same ordering invariant StartQuery enforces. An equivalent well-formed call (StartTime before EndTime) still answers 200 (positive control).
Emulator: the status (400) and code (ValidationException) are right, but the message is the crate's own sentence "StartTime must not be later than EndTime: the time frame holds no instant." — built in src/ops/list_internet_events.rs at the `start_bound > end_bound` guard.
Why: a client or test that surfaces, matches or logs AWS's refusal text for an inverted window sees text AWS never emits, so message-keyed error handling and golden-output comparisons do not recognize the refusal; operators reading the message are given a diagnosis in words the service does not use.
Fix: the ValidationException construction in ListInternetEvents' time-window validation (and the same wording wherever the crate shares this ordering rule, so StartQuery's inverted window refuses identically).
Traces: internetmonitor.start-query-invalid-time-range-rejected.

Neither open internetmonitor issue covers these three: 20261029T171856Z-internetmonitor-monitor-and-query-state-machine-diverges-wrong.md covers StartQuery's empty-Resources gate, UpdateMonitor/GetMonitor state and members and DeleteMonitor's missing 404; 20261029T171856Z-internetmonitor-refusal-bodies-omit-the-code-member-on.md covers the error envelope's missing `code` member. The replay reported no owner and no stub filing for any of these three checks.

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1): 18/21 passed (85.7%), internetmonitor 18/21, 3 mismatched, 0 errored, 0 skipped. Expected = AWS, actual = emulator.

1. [mismatch] internetmonitor.get-query-results-schema-for-empty-monitor
   After CreateMonitor(no Resources) reaches ACTIVE and StartQuery is refused BadRequestException "Invalid request body" for both MEASUREMENTS and OVERALL_TRAFFIC_SUGGESTIONS (both of those assertions PASSED), get_query_results(MonitorName=<monitor>, QueryId="00000000-0000-4000-8000-000000000000") gives:
   AssertionError: expected ValidationException for bogus QueryId, got: NotImplemented
   (the check's next assertion requires message == "Invalid query ID provided").
   Source confirmation: services/service-com-amazonaws-internetmonitor-internetmonitor20210603/src/ops/get_query_results.rs:4 still carries "//! SCAFFOLDED, NOT IMPLEMENTED" and line 176 returns crate::scaffolded::Scaffolded::not_implemented("GetQueryResults").

2. [mismatch] internetmonitor.list-internet-events-returns-valid-structure
   list_internet_events(StartTime=now-24h, EndTime=now, EventType="AVAILABILITY", MaxResults=10) answers 200 with InternetEvents == []:
   AssertionError: ListInternetEvents returned no AVAILABILITY events in the last 24h
   The check records that the replay against real AWS returned >80 AVAILABILITY events in a 24h window for an account with no monitors, and would then have asserted EventStatus in (ACTIVE, RESOLVED), EventArn == arn:aws:internetmonitor::<account>:internet-event/<EventId> and StartedAt being a timestamp.
   Source confirmation: src/ops/list_internet_events.rs:251 reads store.internet_events; src/state.rs documents the store as "empty until something records an event" and record_internet_event as "No operation of this service calls it" — nothing seeds it.

3. [mismatch] internetmonitor.start-query-invalid-time-range-rejected
   list_internet_events(StartTime=now+1h, EndTime=now, MaxResults=10): code ValidationException matched; message did not:
   AssertionError: unexpected message for StartTime>EndTime: 'StartTime must not be later than EndTime: the time frame holds no instant.' (expected 'Invalid time window specified')
   Source confirmation: src/ops/list_internet_events.rs:205-215 — the `start_bound > end_bound` guard constructs ValidationException with that emulator-authored sentence.

Model evidence (smithy_query query="operation"/"shape"): GetQueryResults GET /v20210603/Monitors/{MonitorName}/Queries/{QueryId}/Results, errors include ValidationException(400); ListInternetEvents GET /v20210603/InternetEvents, output InternetEvents required, @paginated items InternetEvents, errors include ValidationException(400); InternetEventSummary requires EventId/EventArn/StartedAt(date-time)/ClientLocation/EventType/EventStatus; ValidationException httpError 400 with a single `message` member.

## Steps to Reproduce
Boot the emulator from mainline and point a botocore internetmonitor client at it (region us-west-2), then:

Cause 1: create_monitor(MonitorName=N, TrafficPercentageToMonitor=100, MaxCityNetworksToMonitor=500) with no Resources; poll get_monitor(N) until Status==ACTIVE; call get_query_results(MonitorName=N, QueryId="00000000-0000-4000-8000-000000000000") -> emulator answers HTTP 501 NotImplemented; AWS answers HTTP 400 ValidationException with message "Invalid query ID provided".

Cause 2: list_internet_events(StartTime=now-24h, EndTime=now, EventType="AVAILABILITY", MaxResults=10) with no monitors configured -> emulator answers 200 with InternetEvents == []; AWS returns a non-empty page of AVAILABILITY InternetEventSummary records (EventStatus ACTIVE|RESOLVED, EventArn arn:aws:internetmonitor::<accountId>:internet-event/<EventId>, StartedAt an RFC3339 date-time).

Cause 3: list_internet_events(StartTime=now+1h, EndTime=now, MaxResults=10) -> inspect Error.Message: emulator returns 'StartTime must not be later than EndTime: the time frame holds no instant.' where AWS returns 'Invalid time window specified' (code ValidationException, HTTP 400 in both). Then list_internet_events(StartTime=now-1h, EndTime=now, MaxResults=10) still answers 200 (positive control).

Or replay the checks named in trace_ids with the behavioral suite.
