Issue-Source: trace-sweep-divergence
Issue-Trace: internetmonitor-health-events
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: internetmonitor-health-events internetmonitor-monitor-create-errors internetmonitor-monitor-lifecycle internetmonitor-not-found-errors internetmonitor-query-errors
Issue-Shape-Id: com.amazonaws.internetmonitor#InternetMonitor20210603
Issue-Key: internetmonitor|GetHealthEvent|4xx->4xx|$.code
Issue-Key: internetmonitor|CreateMonitor|4xx->4xx|$.code
Issue-Key: internetmonitor|GetMonitor|4xx->4xx|$.code
Issue-Key: internetmonitor|UpdateMonitor|4xx->4xx|$.code
Issue-Key: internetmonitor|GetQueryStatus|4xx->4xx|$.code
Issue-Key: internetmonitor|StopQuery|4xx->4xx|$.code
Issue-Commit: daf74f430223eff1dc28404b54e4f3c7221767a2

# internetmonitor refusal bodies omit the code member on every modeled error

> _Note: this issue was identified against the code base at commit `daf74f430223eff1dc28404b54e4f3c7221767a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetHealthEvent

## Description
SIX net-new causes, one per refusal path, all the same defect: this service's restJson1 error envelope carries only `message`, with no `code` member naming the error shape. On every one of these steps the HTTP status and the message text already match the recording — the ONLY divergence is the absent discriminator, so a client that reads the error type out of the body sees an untyped error on every refusal this service makes. Fix belongs in ONE place: the error serializer behind this crate's shared error channel (crate::scaffolded / the generated restJson1 error serialization for com.amazonaws.internetmonitor), which must emit the modeled error shape's name as the body's `code` member alongside `message` for every error of every operation. The six causes below are the six recorded instances; closing the serializer closes all six, and each must be re-replayed.

Cause 1 of 6 — GetHealthEvent, unresolvable EventId (fixture internetmonitor-health-events, step task#3).
AWS: Smithy GetHealthEvent models ValidationException (errorKind client, @httpError 400) whose only modeled member is `message`; the `code` key is the service's wire discriminator, not a modeled member, so it is recorded behavior only. The recording answers HTTP 400 with body {"code":"ValidationException","message":"resourceID is invalid: health-event/<MonitorName>/<EventId>"} — fixed text "resourceID is invalid: " followed by the resource path built from the request's MonitorName and EventId.
Emulator: HTTP 400 with the matching message, but $.code is absent (replay detail: "$.code: missing (expected 'ValidationException', got None)").
Why: an SDK or script that switches on the body's error code cannot tell a validation refusal from a conflict or a not-found, so retry/catch logic that keys on ValidationException never fires.
Fix: the service-wide restJson1 error serializer (not this handler).
Traces: internetmonitor-health-events#task#3.

Cause 2 of 6 — CreateMonitor, duplicate monitor name (fixture internetmonitor-monitor-create-errors, step task#2).
AWS: Smithy CreateMonitor models ConflictException (client, @httpError 409, member `message` only), "The requested resource is in use." Recorded: HTTP 409 with $.code == "ConflictException" plus the message.
Emulator: HTTP 409 with the message but no $.code.
Why: a create that races another create is indistinguishable from any other 409 in the body; idempotent-create wrappers that match on ConflictException fall through.
Fix: same serializer.
Traces: internetmonitor-monitor-create-errors#task#2.

Cause 3 of 6 — GetMonitor, name no monitor carries (fixture internetmonitor-monitor-lifecycle, step teardown#9).
AWS: the service answers 404 ResourceNotFoundException ("The request specifies a resource that doesn't exist", member `message` only; the crate itself widens it in through extra_errors on GetMonitor). Recorded: HTTP 404 with $.code == "ResourceNotFoundException" and the message "No resource found for account <accountId> and resource monitor/<MonitorName>" — fixed text "No resource found for account " / " and resource ", with the serving account id and the request's monitor resource path as the variable parts.
Emulator: HTTP 404, message matches (state::monitor_not_found_message), $.code absent.
Why: cleanup and waiter loops key terminal "gone" on the error code in the body and instead see an untyped 404.
Fix: same serializer.
Traces: internetmonitor-monitor-lifecycle#teardown#9, internetmonitor-not-found-errors#task#6.

Cause 4 of 6 — UpdateMonitor, name no monitor carries (fixture internetmonitor-not-found-errors, step task#7).
AWS: Smithy UpdateMonitor models ResourceNotFoundException (client, @httpError 404). Recorded: HTTP 404 with $.code == "ResourceNotFoundException" and the same not-found wording as cause 3.
Emulator: HTTP 404 with matching message, $.code absent.
Why: as cause 3, on the write path.
Fix: same serializer.
Traces: internetmonitor-not-found-errors#task#7.

Cause 5 of 6 — GetQueryStatus, QueryId the monitor does not hold (fixture internetmonitor-query-errors, step task#2).
AWS: Smithy GetQueryStatus models ValidationException (client, @httpError 400). Recorded: HTTP 400 with $.code == "ValidationException" and the query-id refusal message (fixed text, no per-run values).
Emulator: HTTP 400 with matching message, $.code absent.
Why: polling code cannot distinguish "bad query id" from a throttle or a server fault by code.
Fix: same serializer.
Traces: internetmonitor-query-errors#task#2.

Cause 6 of 6 — StopQuery, QueryId the monitor does not hold (fixture internetmonitor-query-errors, step task#3).
AWS: Smithy StopQuery models ValidationException (client, @httpError 400). Recorded: HTTP 400 with $.code == "ValidationException" and the query-id refusal message.
Emulator: HTTP 400 with matching message, $.code absent.
Why: as cause 5, on the cancel path.
Fix: same serializer.
Traces: internetmonitor-query-errors#task#3.

No open issue covers this: the five existing internetmonitor issues (including "internetmonitor: 7 refusal-path causes — wrong error shapes…") are all in the resolved folder, and none of these clusters was reported as owned or stub-blocked by the replay.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, score 61.8%, 22 of 68 steps mismatched). Six clusters, verdict=mismatch, each with expected_status == actual_status and a single diverging key:
- internetmonitor-health-events#task#3 GetHealthEvent 400->400: "$.code: missing (expected 'ValidationException', got None)"
- internetmonitor-monitor-create-errors#task#2 CreateMonitor 409->409: "$.code: missing (expected 'ConflictException', got None)"
- internetmonitor-monitor-lifecycle#teardown#9 GetMonitor 404->404: "$.code: missing (expected 'ResourceNotFoundException', got None)" (also internetmonitor-not-found-errors#task#6)
- internetmonitor-not-found-errors#task#7 UpdateMonitor 404->404: "$.code: missing (expected 'ResourceNotFoundException', got None)"
- internetmonitor-query-errors#task#2 GetQueryStatus 400->400: "$.code: missing (expected 'ValidationException', got None)"
- internetmonitor-query-errors#task#3 StopQuery 400->400: "$.code: missing (expected 'ValidationException', got None)"
Expected (recorded AWS): body = {"code": "<ErrorShapeName>", "message": "<the error's wording>"}.
Actual (emulator): body carries the message only; no `code` member for any error of any operation.
Model grounding: smithy_query(service_info) -> protocols [restJson1]; ValidationException/ResourceNotFoundException/ConflictException each declare exactly one member, `message`, with httpError 400/404/409 — so `code` is a wire-envelope member the serializer must add, not a handler field.

## Steps to Reproduce
1. Boot the emulator from mainline and replay trace set simparity-wire-aws-v1.
2. GET /v20210603/Monitors/{MonitorName}/HealthEvents/{EventId} for an EventId the monitor does not hold (fixture internetmonitor-health-events, step task#3) and read the 400 body: it has `message` but no `code`.
3. Repeat for the other five recorded refusals: POST /v20210603/Monitors with a name already taken (409), GET and PATCH /v20210603/Monitors/{MonitorName} for an absent monitor (404), and GET .../Queries/{QueryId}/Status and DELETE .../Queries/{QueryId} with an unknown QueryId (400).
4. Each response has the right status and message and no `code` member.
