Issue-Source: trace-sweep-divergence
Issue-Trace: logs-agentcore-traj-vended-groups-empty-reference-path
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-agentcore-traj-vended-groups-empty-reference-path logs-osdocstat-traj-audit-log-json-filter-patterns logs-osdocstat-traj-log-group-arn-forms logs-refarch-rekreup-handler-log-filter-mistakes logs-refarch-s3ev-handler-log-group-absent logs-wsbe-traj-handler-log-filter-pattern-mistakes logsrd-filter-log-events-read logsval-filter-log-events-name-and-identifier-exclusive logsval-filter-log-events-name-or-identifier-required logsval-filter-log-events-start-from-head-false-old-start-time-accepted logsval-filter-log-events-stream-names-and-prefix-exclusive logs-onyx-traj-search-the-handler-log-for-skips
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|FilterLogEvents|2xx->2xx|$.searchStatistics
Issue-Key: logs|FilterLogEvents|2xx->2xx|$.nextToken
Issue-Key: logs|FilterLogEvents|4xx->2xx|$.__type
Issue-Key: logs|FilterLogEvents|4xx->2xx|$.message
Issue-Key: logs|FilterLogEvents|4xx->2xx|status
Issue-Key: logs|FilterLogEvents|2xx->4xx|$.events
Issue-Key: logs|FilterLogEvents|2xx->4xx|$.searchStatistics
Issue-Key: logs|FilterLogEvents|2xx->4xx|$.searchedLogStreams
Issue-Key: logs|FilterLogEvents|2xx->4xx|status
Issue-Commit: 018054ec3370c5c9f981d4e69b97a5a5db6eaccb

# logs FilterLogEvents response fields and filter validation diverge

> _Note: this issue was identified against the code base at commit `018054ec3370c5c9f981d4e69b97a5a5db6eaccb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
FilterLogEvents

## Description
CloudWatch Logs `FilterLogEvents` is implemented in this emulator (it answers 200), but it diverges from AWS in four independent ways: it omits two members AWS always serializes on a successful search, it accepts a filter pattern AWS refuses, and it refuses a descending-scan request AWS accepts. This bundle carries 4 CAUSES; all four must be closed before the issue is submitted.

NOTE ON RELATED ISSUES: the open issue `20261022T045628Z-cloudwatch-logs-filterlogevents-stub.md` ("Implement cloudwatch-logs FilterLogEvents: unimplemented scaffold stub") asks for the operation to EXIST; it carries no replay key for any of the steps below and does not cover these fidelity gaps. The handler is no longer a scaffold on this branch.

=== Cause 1 of 4: a successful FilterLogEvents omits `searchStatistics` ===
AWS: recorded behavior only — the Smithy model's `FilterLogEventsResponse` declares only `events`, `searchedLogStreams` and `nextToken` (Smithy: FilterLogEvents output = events/searchedLogStreams/nextToken), so the field is not modeled. The recordings show AWS returning, on every HTTP 200 FilterLogEvents response, an additional `searchStatistics` object whose members are the counts of log streams the search covered: `approximateTotalLogStreamCount` (how many streams the search was spread over) and `completedLogStreamCount` (how many of those were searched to completion). In the recorded steps the search covered no stream, so both counts are 0; the requirement is that the object is present with both members carrying the counts for the request's own stream selection, not that they are 0.
Emulator: the handler returns the response without that member at all — replay reports `$.searchStatistics: missing (expected {'approximateTotalLogStreamCount': 0, 'completedLogStreamCount': 0}, got None)` on an otherwise matching 200. This one divergence shape covers 21 recorded steps across 10 fixtures.
Why: a client that reads `searchStatistics` to tell a complete search from a partial one (the only signal that a page stopped early because a stream was not fully scanned) gets a missing key and either crashes on the dereference or silently concludes every search was complete.
Fix: the output construction of the FilterLogEvents handler (the `FilterLogEventsOutput { events, searched_log_streams, next_token }` it builds), plus whatever counting the scan must carry to populate the two members.
Traces: logs-agentcore-traj-vended-groups-empty-reference-path#task#9 (representative); also logs-osdocstat-traj-audit-log-json-filter-patterns#task#4..#8 and #11, logs-osdocstat-traj-log-group-arn-forms#task#5, logs-refarch-rekreup-handler-log-filter-mistakes#task#4/#6/#7, logs-refarch-s3ev-handler-log-group-absent#task#5, logs-wsbe-traj-handler-log-filter-pattern-mistakes#task#6, logsrd-filter-log-events-read#task#4/#5/#6, logsval-filter-log-events-name-and-identifier-exclusive#task#3, logsval-filter-log-events-name-or-identifier-required#task#3, logsval-filter-log-events-start-from-head-false-old-start-time-accepted#task#4, logsval-filter-log-events-stream-names-and-prefix-exclusive#task#3/#4.

=== Cause 2 of 4: a FilterLogEvents page that has more results omits `nextToken` ===
AWS: Smithy: FilterLogEvents is `@paginated` with inputToken=nextToken, outputToken=nextToken, pageSize=limit; the operation's own documentation states "If the results include a nextToken, there might be more log events available [...] If the results don't include a nextToken, then pagination is finished" and "A returned page might only be partially full, or even empty. [...] Partially full or empty pages don't necessarily mean that pagination is finished". The recordings show AWS returning a `nextToken` (an opaque continuation string) on the recorded searches, together with the `searchStatistics` object of Cause 1.
Emulator: the handler returns neither — replay reports `$.nextToken: missing (expected '<shaped-not-asserted>', got None)` and `$.searchStatistics: missing (...)` on a 200. The emulator emits a token only when its own matched-event vector has an element past the page it just served, so a search that AWS still considers resumable terminates pagination here.
Why: a client paginating FilterLogEvents stops at the first page and silently loses every later event, because absence of `nextToken` is defined by AWS as "pagination is finished".
Fix: the pagination/cursor logic of the FilterLogEvents handler (the `next_token` computation at the end of `handle`), which must emit a continuation whenever the scan is not known to be exhausted rather than only when a further matched event is already materialized.
Traces: logs-onyx-traj-search-the-handler-log-for-skips#task#5 (representative; also #6, #7), logs-wsbe-traj-handler-log-filter-pattern-mistakes#task#5 and #7.

=== Cause 3 of 4: an unparsable filterPattern term is accepted instead of refused ===
AWS: Smithy: FilterLogEvents.filterPattern targets FilterPattern (@length min 0, max 1024) and the operation models `InvalidParameterException` (HTTP 400, "A parameter is specified incorrectly"). The recorded response for a filter pattern containing a bare double-quote character is HTTP 400 with `__type` `InvalidParameterException` and message `Invalid character(s) in term '"'`. In that message the fixed text is `Invalid character(s) in term '` ... `'`, and the quoted value between the single quotes is the offending term taken verbatim from the request's `filterPattern` — here a double-quote character. A pattern that does not parse is a bad request, not a search that matches nothing.
Emulator: the request is answered HTTP 200 with a result body — replay reports `status: expected 400, got 200`, `$.__type: missing (expected 'InvalidParameterException', got None)`, `$.message: missing (expected 'Invalid character(s) in term \'"\'', got None)`. The handler's `FilterPattern::parse` accepts the term rather than rejecting it, so no refusal is raised.
Why: a client that mistypes a filter pattern gets a confident, empty-looking success instead of the 400 that tells it the pattern is malformed, so a monitoring query silently reports "no matching events" when it never ran the intended search.
Fix: the filter-pattern parser/validation reached from the FilterLogEvents handler (the `FilterPattern::parse` call whose error is mapped to InvalidParameterException), which must reject terms carrying characters the pattern grammar does not admit and word the refusal as AWS does.
Traces: logs-onyx-traj-search-the-handler-log-for-skips#task#8 (representative), logs-osdocstat-traj-audit-log-json-filter-patterns#task#10, logs-refarch-rekreup-handler-log-filter-mistakes#task#2, logs-wsbe-traj-handler-log-filter-pattern-mistakes#task#4, logsrd-filter-log-events-read#task#8.

=== Cause 4 of 4: startFromHead=false with a pre-2024 startTime is refused instead of answered ===
AWS: recorded behavior only for the outcome. The model's prose for `FilterLogEventsRequest$startFromHead` says "Setting startFromHead to false is supported only when startTime is on or after Jan 1, 2024 00:00:00 UTC. A request with startFromHead set to false and a startTime before this date returns an InvalidParameterException", but the recording for exactly that request shape is HTTP 200 with an empty result: `events: []`, `searchedLogStreams: []` and the `searchStatistics` object of Cause 1. The recorded response is the requirement: the descending scan is accepted and answered.
Hypothesis: the documented restriction is not enforced by the service for this request shape (for example it is not applied when the search resolves to no log stream), so the emulator's unconditional pre-check is stricter than the service.
Emulator: the request is refused with HTTP 400 — replay reports `status: expected 200, got 400` and the three expected success members missing (`$.events`, `$.searchStatistics`, `$.searchedLogStreams`). The handler raises its `DESCENDING_SCAN_UNSUPPORTED` InvalidParameterException whenever `startFromHead` is false and `startTime` is before its hardcoded Jan 1 2024 epoch constant.
Why: a client that paginates newest-first over a time range that begins before 2024 — a normal historical query — is rejected outright where AWS answers it, so the workload fails rather than returning its (possibly empty) page.
Fix: the input validation of the FilterLogEvents handler (the descending-scan epoch pre-check guarding `DESCENDING_SCAN_UNSUPPORTED`), which must not refuse the request shape the recording shows AWS accepting.
Traces: logsval-filter-log-events-start-from-head-false-old-start-time-accepted#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, 1122 steps, 894 passed, score 79.7%). The four FilterLogEvents clusters below are the only divergences in the corpus that are neither blocked by an unimplemented operation (stub_path empty) nor owned by an open issue (owned_by empty).

Cause 1 — logs-agentcore-traj-vended-groups-empty-reference-path#task#9, FilterLogEvents, verdict=mismatch, expected 200 / actual 200:
  $.searchStatistics: missing (expected {'approximateTotalLogStreamCount': 0, 'completedLogStreamCount': 0}, got None)
  divergence_key: service=logs|FilterLogEvents|status=200->200|keys=$.searchStatistics (21 affected steps)

Cause 2 — logs-onyx-traj-search-the-handler-log-for-skips#task#5, FilterLogEvents, expected 200 / actual 200:
  $.nextToken: missing (expected '<shaped-not-asserted>', got None)
  $.searchStatistics: missing (expected {'approximateTotalLogStreamCount': 0, 'completedLogStreamCount': 0}, got None)
  divergence_key: service=logs|FilterLogEvents|status=200->200|keys=$.nextToken,$.searchStatistics (5 affected steps)

Cause 3 — logs-onyx-traj-search-the-handler-log-for-skips#task#8, FilterLogEvents, expected 400 / actual 200:
  status: expected 400, got 200
  $.__type: missing (expected 'InvalidParameterException', got None)
  $.message: missing (expected 'Invalid character(s) in term \'"\'', got None)
  divergence_key: service=logs|FilterLogEvents|status=400->200|keys=$.__type,$.message,status (5 affected steps)

Cause 4 — logsval-filter-log-events-start-from-head-false-old-start-time-accepted#task#3, FilterLogEvents, expected 200 / actual 400:
  status: expected 200, got 400
  $.events: missing (expected [], got None)
  $.searchStatistics: missing (expected {'approximateTotalLogStreamCount': 0, 'completedLogStreamCount': 0}, got None)
  $.searchedLogStreams: missing (expected [], got None)
  divergence_key: service=logs|FilterLogEvents|status=200->400|keys=$.events,$.searchStatistics,$.searchedLogStreams,status (1 affected step)

Model evidence (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/smithy-model/com-amazonaws-cloudwatchlogs-logs-20140328.json): FilterLogEvents output members are events/searchedLogStreams/nextToken only (searchStatistics unmodeled — recorded behavior); @paginated inputToken=nextToken outputToken=nextToken pageSize=limit; errors InvalidParameterException (400), ResourceNotFoundException (400), ServiceUnavailableException (500); filterPattern @length 0..1024.
Handler evidence (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/filter_log_events.rs): line 200 `DESCENDING_SCAN_EPOCH_MS` and lines 454-460 the unconditional descending-scan refusal (Cause 4); lines 464-466 `FilterPattern::parse(...).map_err(invalid)` (Cause 3); lines 555-571 the output construction with only events/searched_log_streams/next_token (Causes 1 and 2).

## Steps to Reproduce
Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1, then:

Cause 1: replay logs-agentcore-traj-vended-groups-empty-reference-path step task#9 — call FilterLogEvents on an existing log group and inspect the 200 body: `searchStatistics` is absent where AWS returns {approximateTotalLogStreamCount, completedLogStreamCount}.
Cause 2: replay logs-onyx-traj-search-the-handler-log-for-skips step task#5 — call FilterLogEvents on a group whose search AWS reports as resumable and observe that neither `nextToken` nor `searchStatistics` is returned, so pagination terminates on the first page.
Cause 3: replay logs-onyx-traj-search-the-handler-log-for-skips step task#8 — call FilterLogEvents with a filterPattern containing a bare double-quote term and observe HTTP 200 with a result body instead of HTTP 400 InvalidParameterException "Invalid character(s) in term '\"'".
Cause 4: replay logsval-filter-log-events-start-from-head-false-old-start-time-accepted step task#3 — call FilterLogEvents with startFromHead=false and a startTime before 2024-01-01T00:00:00Z and observe HTTP 400 InvalidParameterException instead of HTTP 200 with events: [], searchedLogStreams: [] and searchStatistics.
