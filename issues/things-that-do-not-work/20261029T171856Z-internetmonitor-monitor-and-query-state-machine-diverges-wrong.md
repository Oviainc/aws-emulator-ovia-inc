Issue-Source: trace-sweep-divergence
Issue-Trace: internetmonitor-query-workflow
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: internetmonitor-query-workflow internetmonitor-health-events internetmonitor-monitor-lifecycle internetmonitor-monitor-status-toggle internetmonitor-monitor-tags-roundtrip internetmonitor-not-found-errors
Issue-Shape-Id: com.amazonaws.internetmonitor#InternetMonitor20210603
Issue-Key: internetmonitor|StartQuery|2xx->4xx|status
Issue-Key: internetmonitor|UpdateMonitor|2xx->2xx|$.Status
Issue-Key: internetmonitor|GetMonitor|2xx->2xx|$.CreatedAt
Issue-Key: internetmonitor|GetMonitor|2xx->2xx|$.InternetMeasurementsLogDelivery
Issue-Key: internetmonitor|GetMonitor|2xx->2xx|$.ModifiedAt
Issue-Key: internetmonitor|GetMonitor|2xx->2xx|$.ProcessingStatusInfo
Issue-Key: internetmonitor|GetMonitor|2xx->2xx|$.ProcessingStatus
Issue-Key: internetmonitor|DeleteMonitor|4xx->2xx|$
Issue-Key: internetmonitor|DeleteMonitor|4xx->2xx|status
Issue-Commit: daf74f430223eff1dc28404b54e4f3c7221767a2

# internetmonitor monitor and query state machine diverges: wrong statuses, dropped members, missing and invented refusals

> _Note: this issue was identified against the code base at commit `daf74f430223eff1dc28404b54e4f3c7221767a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQuery

## Description
FIVE net-new causes in the monitor/query lifecycle: the emulator invents one refusal AWS does not make, omits one AWS does make, reports the wrong Status and ProcessingStatus, and drops two modeled GetMonitor members while serializing timestamps in the wrong JSON type. All five must be closed and re-replayed.

Cause 1 of 5 — StartQuery refuses a monitor whose Resources set is empty (fixture internetmonitor-query-workflow, step task#2).
AWS: Smithy StartQuery = POST /v20210603/Monitors/{MonitorName}/Queries, required input StartTime/EndTime/QueryType, output QueryId (required); modeled errors are AccessDeniedException(403), LimitExceededException(403), ThrottlingException(429), ValidationException(400), InternalServerException(500). Nothing in the model conditions acceptance on how many Resources the monitor holds. Recorded behavior: HTTP 200 with a QueryId for exactly this request.
Emulator: HTTP 400 — src/ops/start_query.rs refuses with BadRequestException and the fixed message "Invalid request body" whenever monitor.resources.is_empty(), an admission rule the service does not have.
Why: the whole query interface is unreachable for such a monitor — the client never gets a QueryId, and every dependent call (GetQueryStatus, StopQuery, query results) is unrunnable; this invented refusal is what left 4 steps blocked in this replay.
Fix: StartQuery's handler validation — drop the empty-Resources gate and record the query.
Traces: internetmonitor-query-workflow#task#2, internetmonitor-query-workflow#task#5.

Cause 2 of 5 — UpdateMonitor reports the requested Status instead of PENDING (fixture internetmonitor-health-events, step teardown#5; 8 steps across 6 fixtures).
AWS: Smithy UpdateMonitorOutput.Status is required and targets MonitorConfigState (PENDING | ACTIVE | INACTIVE | ERROR). Recorded behavior: a PATCH that sets the config state answers HTTP 200 with Status "PENDING" — the requested state is applied asynchronously, so the response reports the in-flight state, not the requested terminal one.
Emulator: HTTP 200 with Status "INACTIVE" — src/ops/update_monitor.rs assigns monitor.status = requested status, clears the owed transition (active_at = None) and echoes it back.
Why: a client that polls for PENDING -> settled sees the change already final, so waiters built on the documented state machine never observe the transition and cannot detect a configuration that fails to apply.
Fix: UpdateMonitor's handler plus the monitor record in src/state.rs — report PENDING and leave the clock to owe the settled state, as CreateMonitor already does via MONITOR_ACTIVATION_DELAY/settle.
Traces: internetmonitor-health-events#teardown#5, internetmonitor-monitor-lifecycle#task#4, internetmonitor-monitor-lifecycle#teardown#6, internetmonitor-monitor-status-toggle#task#3, internetmonitor-monitor-status-toggle#task#5, internetmonitor-monitor-status-toggle#teardown#7, internetmonitor-monitor-tags-roundtrip#teardown#8, internetmonitor-query-workflow#teardown#9.

Cause 3 of 5 — GetMonitor drops InternetMeasurementsLogDelivery and ProcessingStatusInfo and serializes CreatedAt/ModifiedAt as JSON numbers (fixture internetmonitor-monitor-lifecycle, step task#2).
AWS: Smithy GetMonitorOutput declares InternetMeasurementsLogDelivery (structure, member S3Config) and ProcessingStatusInfo (String), and CreatedAt/ModifiedAt as Timestamp with @timestampFormat date-time — so AWS serializes both instants as RFC3339 date-time STRINGS. Recorded behavior: the response carries InternetMeasurementsLogDelivery present as a structure whose S3Config is absent (i.e. an empty log-delivery configuration is reported, not an omitted member) and ProcessingStatusInfo with AWS's fixed sentence "The monitor is OK and is collecting data-points to produce insights" (no variable part).
Emulator: both members are omitted entirely, and CreatedAt/ModifiedAt come back as epoch-seconds numbers (e.g. 1793294089.8592033) from state::wire_timestamp. The clock VALUE differs per run and is not the finding; the JSON type is — a number where the model requires a date-time string.
Why: a client reading the monitor's log-delivery configuration finds nothing to read, a client surfacing processing health has no info text, and an SDK or parser that expects a date-time string fails or mis-decodes both timestamps.
Fix: GetMonitor's output projection and the monitor record in src/state.rs (carry the log-delivery structure and the processing-status info text), plus the timestamp serialization so a date-time-formatted Timestamp is written as an RFC3339 string.
Traces: internetmonitor-monitor-lifecycle#task#2, internetmonitor-monitor-lifecycle#task#5, internetmonitor-monitor-status-toggle#task#6.

Cause 4 of 5 — ProcessingStatus is derived from the monitor's config state (fixture internetmonitor-monitor-status-toggle, step task#4).
AWS: Smithy GetMonitorOutput.ProcessingStatus targets MonitorProcessingStatusCode (OK | INACTIVE | COLLECTING_DATA | INSUFFICIENT_DATA | FAULT_SERVICE | FAULT_ACCESS_CLOUDWATCH) and is documented as "the health of the data processing for the monitor" — a property of the data pipeline, not of the configuration state. Recorded behavior at this step: ProcessingStatus "COLLECTING_DATA" with ProcessingStatusInfo "The monitor is OK and is collecting data-points to produce insights".
Emulator: ProcessingStatus "INACTIVE" — src/state.rs MonitorRecord::processing_status_for maps config state INACTIVE straight onto processing status INACTIVE, and UpdateMonitor recomputes it from the Status the request carried.
Why: a dashboard or alarm that reads ProcessingStatus to decide whether measurements are flowing reads a value that only restates the config state, so it reports a data-processing fault that AWS does not report.
Fix: the monitor record's processing-status derivation in src/state.rs (and UpdateMonitor's recomputation of it).
Traces: internetmonitor-monitor-status-toggle#task#4.

Cause 5 of 5 — DeleteMonitor on a name no monitor carries answers 200 (fixture internetmonitor-not-found-errors, step task#8).
AWS: recorded behavior — HTTP 404 with body {"code":"ResourceNotFoundException","message":"No resource found for account <accountId> and resource monitor/<MonitorName>"}. Fixed text: "No resource found for account " and " and resource "; variable parts: the serving account id and the request's monitor resource path monitor/<MonitorName>. Smithy DeleteMonitor itself lists only AccessDenied/InternalServer/Throttling/ValidationException, so this 404 shape is recorded behavior that the crate already widens in elsewhere (GetMonitor's extra_errors) and already words in one place (state::monitor_not_found_message).
Emulator: HTTP 200 with an empty body — src/ops/delete_monitor.rs removes the monitor unconditionally and never tests presence, so a delete of something that was never there succeeds.
Why: a caller cannot tell "I deleted it" from "it was never there"; teardown and reconciliation loops treat a typo'd or already-reclaimed monitor name as a successful delete and silently skip the real resource.
Fix: DeleteMonitor's handler — refuse an unknown MonitorName with the service's 404 not-found shape built from state::monitor_not_found_message (the body must also carry the `code` member; that envelope defect is filed separately for this service).
Traces: internetmonitor-not-found-errors#task#8.

No open issue covers these: all five existing internetmonitor issues are in the resolved folder, and the replay reported no owner and no stub for any of these clusters.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, score 61.8%, 22/68 steps mismatched, 4 steps blocked). Five mismatch clusters:
1. StartQuery internetmonitor-query-workflow#task#2 (and #task#5): "status: expected 200, got 400" — recorded AWS returns 200 + QueryId; emulator refuses with BadRequestException "Invalid request body" because the monitor holds no Resources (src/ops/start_query.rs, the monitor.resources.is_empty() gate). Downstream query steps are the 4 blocked steps.
2. UpdateMonitor internetmonitor-health-events#teardown#5, 200->200: "$.Status: value_mismatch (expected 'PENDING', got 'INACTIVE')"; 8 affected steps across internetmonitor-health-events, -monitor-lifecycle, -monitor-status-toggle, -monitor-tags-roundtrip, -query-workflow. Code: src/ops/update_monitor.rs sets monitor.status = input.status, monitor.active_at = None, returns monitor.status.
3. GetMonitor internetmonitor-monitor-lifecycle#task#2, 200->200: "$.InternetMeasurementsLogDelivery: missing (expected {'S3Config': None}, got None)", "$.ProcessingStatusInfo: missing (expected 'The monitor is OK and is collecting data-points to produce insights', got None)", plus CreatedAt/ModifiedAt returned as 1793294089.8592033 where the model declares @timestampFormat date-time (per-run instant values are not the claim; the JSON number type is). Code: src/ops/get_monitor.rs output projection, state::wire_timestamp.
4. GetMonitor internetmonitor-monitor-status-toggle#task#4, 200->200: "$.ProcessingStatus: value_mismatch (expected 'COLLECTING_DATA', got 'INACTIVE')" plus the same two missing members. Code: src/state.rs MonitorRecord::processing_status_for maps config INACTIVE -> processing INACTIVE.
5. DeleteMonitor internetmonitor-not-found-errors#task#8: "status: expected 404, got 200" and "$: empty_body (expected {'code': 'ResourceNotFoundException', 'message': 'No resource found for account <accountId> and resource monitor/<MonitorName>'}, got {})". Code: src/ops/delete_monitor.rs removes without a presence check.
Model grounding (smithy_query on com-amazonaws-internetmonitor-internetmonitor20210603): StartQuery output QueryId required with no Resources precondition; UpdateMonitorOutput.Status -> MonitorConfigState [PENDING, ACTIVE, INACTIVE, ERROR]; GetMonitorOutput.{InternetMeasurementsLogDelivery, ProcessingStatusInfo, ProcessingStatus -> MonitorProcessingStatusCode}; CreatedAt/ModifiedAt Timestamp timestampFormat date-time.

## Steps to Reproduce
1. Boot the emulator from mainline and replay trace set simparity-wire-aws-v1.
2. Cause 1: create a monitor with no Resources, then POST /v20210603/Monitors/{MonitorName}/Queries with StartTime/EndTime/QueryType (fixture internetmonitor-query-workflow, step task#2) — emulator answers 400 "Invalid request body" where the recording answers 200 with a QueryId; the dependent status/stop steps are then blocked.
3. Cause 2: PATCH /v20210603/Monitors/{MonitorName} with a Status change (internetmonitor-health-events, step teardown#5) — response Status is the requested state instead of PENDING.
4. Cause 3: GET /v20210603/Monitors/{MonitorName} (internetmonitor-monitor-lifecycle, step task#2) — InternetMeasurementsLogDelivery and ProcessingStatusInfo are absent and CreatedAt/ModifiedAt are JSON numbers rather than date-time strings.
5. Cause 4: toggle the monitor's config state, then GET it (internetmonitor-monitor-status-toggle, step task#4) — ProcessingStatus is INACTIVE where the recording reports COLLECTING_DATA.
6. Cause 5: DELETE /v20210603/Monitors/{MonitorName} for a name no monitor carries (internetmonitor-not-found-errors, step task#8) — 200 with an empty body instead of 404 ResourceNotFoundException.
