Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore.browser-session-lifecycle
Issue-Shape-Id: com.amazonaws.bedrockagentcore#AmazonBedrockAgentCore
Issue-Key: bedrock-agentcore.browser-session-lifecycle|run#14|KeyError|streams = got["streams"]
Issue-Commit: 52f91172eb2f0209f53a5585da2848c5cfe26576

# Browser session streams are never reported: StartBrowserSession/GetBrowserSession omit the required `streams` structure, so a READY session exposes no automation or live-view endpoint

> _Note: this issue was identified against the code base at commit `52f91172eb2f0209f53a5585da2848c5cfe26576`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBrowserSession

## Description
Cause 1 of 1 — browser session stream representation is absent from both the start and the read of a session (failing check: bedrock-agentcore.browser-session-lifecycle).

AWS: Smithy (com.amazonaws.bedrockagentcore) models `streams` (shape `BrowserSessionStream`) on BOTH `StartBrowserSessionResponse` and `GetBrowserSessionResponse`. Inside `BrowserSessionStream`, `automationStream` is **required** (`AutomationStream` = `streamEndpoint` **required**, `BrowserStreamEndpoint` len 10..512, plus `streamStatus` **required**, enum `AutomationStreamStatus` = ENABLED | DISABLED) and `liveViewStream` carries `streamEndpoint` (len 10..512). The operation's own modeled documentation states the response "includes information about the session's viewport configuration, timeout settings, and stream endpoints" and that GetBrowserSession "returns the session's configuration, current status, associated streams, and metadata". So a session in status READY (the only non-TERMINATED value `BrowserSessionStatus` models) carries a populated `streams` with a non-empty automation `streamEndpoint` and `streamStatus` = ENABLED, and a live-view `streamEndpoint`; the viewPort/sessionTimeoutSeconds echo and `createdAt` the same check asserts already pass, so this is the only member of that representation missing.

Emulator: `streams` is omitted entirely from both responses. `StartBrowserSession` builds its output with `streams: None` (both on the fresh-create path and on the idempotency-token replay path), and `GetBrowserSession` likewise sets `streams: None`, with its module doc declaring the member deliberately unreported because "an automation stream is a WebSocket endpoint no process here serves". Replay: after StartBrowserSession(browserIdentifier=aws.browser.v1, sessionTimeoutSeconds=600, viewPort={1280,900}) succeeds and GetBrowserSession returns sessionId/browserIdentifier/status=READY/sessionTimeoutSeconds/viewPort correctly, reading the response's `streams` raises `KeyError: 'streams'` — the key is not present in the deserialized response at all.

Why: the stream endpoints ARE the browser tool's usable surface. A client that starts a browser session in order to drive it (connect a CDP/WebSocket automation client to `streams.automationStream.streamEndpoint`, or embed `streams.liveViewStream.streamEndpoint` for a live view) gets a KeyError / missing-field on a member AWS marks required inside a structure it always returns for a READY session. Every automation and live-view code path built on real AgentCore browser sessions is unrunnable against the emulator even though the session itself reports READY, and a client cannot even tell whether the stream is ENABLED or DISABLED. Omitting a `@required` member of a returned structure also makes the response unrepresentable in strictly-typed SDKs.

Fix: the browser-session state record and the two handlers' output construction. The stored session record (services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/src/state.rs, `BrowserSessionRecord`) should materialize and persist the session's stream endpoints at start time (so start and get cannot disagree, exactly as the handler comment says of the rest of the record), and both `start_browser_session` and `get_browser_session` should serialize them into `streams` with `automationStream.streamEndpoint` non-empty (10..512 chars), `automationStream.streamStatus` = ENABLED while the session is READY, and `liveViewStream.streamEndpoint` populated. The concrete host/port/URL of those endpoints is environment-specific and is not the requirement; their presence, non-emptiness and the ENABLED status for a READY session are. (A session the clock has moved to TERMINATED may report its automation stream DISABLED.)

Traces: bedrock-agentcore.browser-session-lifecycle — fails at `streams = got["streams"]` with `KeyError: 'streams'` (check_bedrock_agentcore_browser_session_lifecycle.py line 48), immediately after the sessionId/browserIdentifier/status/sessionTimeoutSeconds/viewPort assertions pass. Code citations: src/ops/get_browser_session.rs lines 12-14 (doc declaring the member unreported) and line 271 (`streams: None`); src/ops/start_browser_session.rs lines 277 and 296 (`streams: None`).

## Evidence
Expected (AWS / Smithy model com.amazonaws.bedrockagentcore): GetBrowserSession on a READY session returns 200 with `streams` present: {"automationStream": {"streamEndpoint": "<non-empty endpoint, 10..512 chars>", "streamStatus": "ENABLED"}, "liveViewStream": {"streamEndpoint": "<non-empty endpoint>"}}. `BrowserSessionStream.automationStream` is @required, and `AutomationStream.streamEndpoint` and `AutomationStream.streamStatus` are both @required. StartBrowserSession returns the same `streams` member.

Actual (emulator): both responses omit the key. GetBrowserSession 200 body contains browserIdentifier, sessionId, createdAt, viewPort, sessionTimeoutSeconds=600, status=READY, lastUpdatedAt — and NO `streams` key, so the check raises:
  File "check_bedrock_agentcore_browser_session_lifecycle.py", line 48, in run
    streams = got["streams"]
  KeyError: 'streams'
Source of the divergence (not a request-specific value): services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/src/ops/get_browser_session.rs:271 `streams: None,` with the module doc at lines 12-14 stating `streams` is intentionally not reported; services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/src/ops/start_browser_session.rs:277 and :296 `streams: None,`.

## Steps to Reproduce
1. Against the emulator's bedrock-agentcore (data plane) endpoint, call StartBrowserSession with browserIdentifier="aws.browser.v1" (the AWS-managed built-in browser tool), sessionTimeoutSeconds=600, viewPort={"width": 1280, "height": 900}. Note the returned sessionId; observe the response has no `streams` key.
2. Call GetBrowserSession(browserIdentifier="aws.browser.v1", sessionId=<that id>). status is READY and viewPort/sessionTimeoutSeconds/createdAt are echoed correctly.
3. Read `streams` from that response: the key is absent (KeyError in botocore's dict), where AWS returns a populated BrowserSessionStream with a required automationStream (non-empty streamEndpoint, streamStatus=ENABLED) and a liveViewStream streamEndpoint.
4. Equivalent replay: run behavioral check bedrock-agentcore.browser-session-lifecycle.
