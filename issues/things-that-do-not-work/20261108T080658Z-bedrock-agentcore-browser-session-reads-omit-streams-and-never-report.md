Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-browser-session-lifecycle
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-browser-session-lifecycle bedrock-agentcore-error-double-stop-and-invoke-terminated bedrock-agentcore-update-browser-stream
Issue-Shape-Id: com.amazonaws.bedrockagentcore#AmazonBedrockAgentCore
Issue-Key: bedrock-agentcore|StartBrowserSession|2xx->2xx|$.streams
Issue-Key: bedrock-agentcore|GetBrowserSession|2xx->2xx|$.streams
Issue-Key: bedrock-agentcore|GetBrowserSession|2xx->2xx|$.status
Issue-Key: bedrock-agentcore|GetBrowserSession|2xx->2xx|$.sessionTimeoutSeconds
Issue-Commit: 824a021fbc81c5b46e0dca2fae74f8c18911ab8d

# Browser session reads omit streams and never report termination or the session's effective timeout (4 causes)

> _Note: this issue was identified against the code base at commit `824a021fbc81c5b46e0dca2fae74f8c18911ab8d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBrowserSession

## Description
FOUR net-new root causes in the bedrock-agentcore (data plane) browser-session representation, all on the same record-and-report path: the session record never materializes its streams, never records a termination, and does not carry the session's effective timeout. A fixer must close all FOUR before submitting.

Cause 1 of 4: StartBrowserSession omits the `streams` structure from its response.
AWS: Smithy (services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/smithy-model/com-amazonaws-bedrockagentcore-amazonbedrockagentcore.json) models StartBrowserSession as PUT /browsers/{browserIdentifier}/sessions/start, @http code 200, output members browserIdentifier/sessionId/createdAt (all @required) plus `streams` (shape BrowserSessionStream). BrowserSessionStream.automationStream is @required (AutomationStream = @required streamEndpoint, BrowserStreamEndpoint @length 10..512, and @required streamStatus, enum AutomationStreamStatus); liveViewStream carries streamEndpoint (@length 10..512). Recorded behavior: a successful start on the built-in browser tool answers 200 carrying `streams` with a populated automationStream (non-empty endpoint, streamStatus ENABLED for a usable session) and a liveViewStream endpoint.
Emulator: the member is absent from the body. src/ops/start_browser_session.rs constructs `streams: None` on both the fresh-create path (line 296) and the clientToken idempotent-replay path (line 277).
Why: the stream endpoints are the only usable surface of a browser session. A client that starts a session in order to drive it (WebSocket/CDP automation against automationStream.streamEndpoint, or an embedded live view) has no endpoint to connect to, and a strictly-typed SDK cannot even represent the response because a @required member of a returned structure is missing.
Fix: the browser-session state record and this handler's output construction — materialize and persist the session's stream endpoints at start time in crate::state (state/browser.rs BrowserSessionRecord) so start and get cannot disagree, and serialize them here. The concrete host/port of an endpoint is environment-specific and is NOT the requirement; its presence, its length bound and the ENABLED status for a usable session are.
Traces: bedrock-agentcore-browser-session-lifecycle#setup#1 (also bedrock-agentcore-error-double-stop-and-invoke-terminated#setup#1).

Cause 2 of 4: GetBrowserSession omits the same `streams` structure.
AWS: Smithy: GetBrowserSession is GET /browsers/{browserIdentifier}/sessions/get, output includes `streams` (BrowserSessionStream) alongside status/viewPort/sessionTimeoutSeconds; the modeled docs say the read "returns the session's configuration, current status, associated streams, and metadata". Recorded behavior: the read of a live session carries automationStream {streamEndpoint, streamStatus ENABLED} and liveViewStream {streamEndpoint}.
Emulator: src/ops/get_browser_session.rs line 271 sets `streams: None`, with the module/inline comment at lines 269-270 declaring the member deliberately unreported because no process here serves an automation stream. The key is therefore absent from every read.
Why: a client that resumes or inspects an existing session cannot obtain the automation or live-view endpoint and cannot tell whether the automation stream is ENABLED or DISABLED, so no automation flow built on real AgentCore browser sessions runs against the emulator even though the session reports READY.
Fix: get_browser_session.rs output construction, reading the endpoints off the stored record introduced for Cause 1. Note: the open behavioral issue 20261108T071214Z-bedrock-agentcore-browser-session-streams-are-never-reported-startbrowsersession-getbrowsersession.md describes this same missing member, but it is a behavioral-corpus issue and carries NO wire replay key, so it does not cover these fixture steps; they must be replayed even if it is closed first.
Traces: bedrock-agentcore-browser-session-lifecycle#task#2.

Cause 3 of 4: a stopped browser session keeps reporting status READY — the record has no notion of a termination.
AWS: Smithy: BrowserSessionStatus models exactly READY | TERMINATED; StopBrowserSession is PUT /browsers/{browserIdentifier}/sessions/stop and its modeled documentation states it "stops the session, releases associated resources, and makes the session unavailable for further use" and that "once stopped, a session cannot be restarted". Recorded behavior: after the stop succeeds, GetBrowserSession answers 200 with status TERMINATED (and the automation stream no longer ENABLED).
Emulator: GetBrowserSession answers 200 with status READY after the session was stopped. The status is derived ONLY from the clock: state/browser.rs BrowserSessionRecord::status_as_of (lines 147-152) returns TERMINATED only when the time-to-live has elapsed, and BrowserSessionRecord (lines 78-104) carries no `stopped_at` at all — unlike CodeInterpreterSessionRecord, which does. So no call can move a browser session to TERMINATED. (StopBrowserSession itself is still a scaffold stub answering 501 — src/ops/stop_browser_session.rs line 196 — whose own stub issue covers only the unwritten handler; this state/read defect would keep masking TERMINATED even after that handler is written, which is why it is filed here.)
Why: a client that stops a session and then polls it is told the session is still READY, so teardown/reconciliation loops never observe the terminal state, treat a released session as usable, and keep driving a session AWS says cannot be used.
Fix: the browser-session state record and the read — add a stopped-at/terminated marker to BrowserSessionRecord in src/state/browser.rs, have status_as_of report TERMINATED from it (as the code-interpreter record already does), and have the stop handler write it.
Traces: bedrock-agentcore-browser-session-lifecycle#teardown#5.

Cause 4 of 4: GetBrowserSession reports the model's @default timeout instead of the session's effective sessionTimeoutSeconds.
AWS: Smithy: StartBrowserSessionRequest.sessionTimeoutSeconds targets BrowserSessionTimeout with @default 3600 and @range 1..28800; GetBrowserSessionResponse.sessionTimeoutSeconds is documented as "The timeout period for the browser session in seconds". Recorded behavior: for the session this fixture starts, every read of it reports sessionTimeoutSeconds 900 — i.e. the effective timeout AWS assigned the session, which is what the read must echo. Hypothesis: the service applies a 900-second effective timeout for this session (either because the start requested 900, or because the service's own default for the built-in browser tool differs from the model's @default); the recording states only the value read back.
Emulator: reports 900's place taken by 3600. src/ops/get_browser_session.rs line 267 echoes `session.session_timeout_seconds`, which state/browser.rs BrowserSessionRecord::started (line 115) copies straight from `input.session_timeout_seconds` — so the stored value is whatever the request walk bound, including the modeled @default 3600 when the request carries no member, and no effective-timeout resolution happens anywhere.
Why: sessionTimeoutSeconds is how a client knows when its session will be reclaimed. Reporting 3600 for a session that is actually reclaimed much earlier makes every keep-alive/renew schedule computed from the read wrong, and the session disappears while the client still believes it has time left. It also makes the emulator's own TTL-derived status transition fire at the wrong instant.
Fix: the start handler plus the stored record (src/ops/start_browser_session.rs, src/state/browser.rs BrowserSessionRecord::started) must persist the session's EFFECTIVE timeout — the requested value when the request carries one, otherwise the value AWS assigns — and get_browser_session.rs must echo that stored value. The requirement is that the read reports the timeout the session actually runs under, not the schema default.
Traces: bedrock-agentcore-update-browser-stream#task#3 (also #task#5).

Why these four are one issue: all four are the same browser-session record-and-report path in this service — what StartBrowserSession persists and what GetBrowserSession serializes back.

## Evidence
All four from the wire replay of simparity-wire-aws-v1 against the booted emulator (trace_set_id=simparity-wire-aws-v1, emulator_up=true, bootstrap_signal=false).

Cause 1 — fixture bedrock-agentcore-browser-session-lifecycle, step setup#1, StartBrowserSession [mismatch], status 200 on both sides:
  $.streams: missing — expected {"automationStream": {"streamEndpoint": "<shaped-not-asserted>", "streamStatus": "ENABLED"}, "liveViewStream": {"streamEndpoint": "<shaped-not-asserted>"}}, got None.
  Cluster also covers bedrock-agentcore-error-double-stop-and-invoke-terminated#setup#1.
  Source (not a request-specific value): src/ops/start_browser_session.rs:277 and :296 `streams: None`.

Cause 2 — fixture bedrock-agentcore-browser-session-lifecycle, step task#2, GetBrowserSession [mismatch], status 200 on both sides:
  $.streams: missing — same expected structure, got None.
  Source: src/ops/get_browser_session.rs:271 `streams: None` with the comment at :269-270 declaring the member unreported.

Cause 3 — fixture bedrock-agentcore-browser-session-lifecycle, step teardown#5, GetBrowserSession [mismatch], status 200 on both sides:
  $.status: expected 'TERMINATED', got 'READY'
  $.streams: missing (as Cause 2)
  Source: src/state/browser.rs:147-152 status_as_of derives the status from expires_at() alone; BrowserSessionRecord (:78-104) has no stopped-at member; src/ops/stop_browser_session.rs:196 still answers 501.

Cause 4 — fixture bedrock-agentcore-update-browser-stream, step task#3 (and task#5), GetBrowserSession [mismatch], status 200 on both sides:
  $.sessionTimeoutSeconds: expected 900, got 3600 (3600 is the BrowserSessionTimeout @default the request walk binds; 900 is the session's effective timeout AWS reports)
  $.streams: missing — expected {"automationStream": {"streamEndpoint": "<shaped-not-asserted>", "streamStatus": "DISABLED"}, "liveViewStream": {"streamEndpoint": "<shaped-not-asserted>"}}, got None. (The DISABLED reading follows an UpdateBrowserStream in the same fixture; that operation is itself still a scaffold stub at src/ops/update_browser_stream.rs:194, so the stream-status value is only reachable once Causes 1-2 give the session a stream to update.)
  Source: src/ops/get_browser_session.rs:267 echoes the stored value; src/state/browser.rs:115 stores `input.session_timeout_seconds` verbatim.

No status, error code or error-message text diverges in these four steps — every one is a 200 whose body omits or misreports a modeled member. Session ids, endpoints, hosts and timestamps in the recordings are per-run values and are not requirements; the presence of `streams`, the ENABLED/DISABLED status, the TERMINATED transition and the echoed effective timeout are.

## Steps to Reproduce
1. Build and boot the emulator from this checkout, then replay the wire corpus simparity-wire-aws-v1.
2. Cause 1 — bedrock-agentcore-browser-session-lifecycle, setup#1: PUT /browsers/aws.browser.v1/sessions/start with a viewPort and a sessionTimeoutSeconds. The 200 body has browserIdentifier/sessionId/createdAt and NO `streams` key; AWS returns a populated BrowserSessionStream (automationStream with a non-empty streamEndpoint and streamStatus ENABLED, plus a liveViewStream streamEndpoint).
3. Cause 2 — same fixture, task#2: GET /browsers/aws.browser.v1/sessions/get?sessionId=<that id>. status is READY and the configuration echoes correctly, but `streams` is again absent.
4. Cause 3 — same fixture, teardown#5: stop the session (PUT /browsers/aws.browser.v1/sessions/stop?sessionId=<id>) and then read it again. The emulator still reports status READY; AWS reports TERMINATED. Note the read can only ever report TERMINATED by letting the time-to-live elapse, because the stored record has no termination marker.
5. Cause 4 — bedrock-agentcore-update-browser-stream, task#3: start a session in that fixture's flow and read it back. The emulator reports sessionTimeoutSeconds 3600 (the BrowserSessionTimeout @default) where AWS reports the session's effective 900, and `streams` is absent so the automation stream's DISABLED status cannot be observed either; task#5 repeats the same read.
