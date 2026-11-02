Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: chime-sdk-meetings.native-media-data-plane-requires-client-sdk
Issue-Shape-Id: com.amazonaws.chimesdkmeetings#ChimeMeetingsSDKService
Issue-Key: chime-sdk-meetings.native-media-data-plane-requires-client-sdk|_tls_websocket_probe#12|ConnectionRefusedError|raw = socket.create_connection((host, port), timeout=timeout)
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# chime-sdk-meetings: the SignalingUrl CreateMeeting advertises in MediaPlacement is not served — TLS connect to the published signaling endpoint is refused

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMeeting

## Description
This issue carries 1 CAUSE, in the chime-sdk-meetings emulator (crate services/service-com-amazonaws-chimesdkmeetings-chimemeetingssdkservice). It is NOT covered by any open chime-sdk-meetings issue: 20261029T175426Z-chime-sdk-meetings-6-net-new-root-causes.md bundles six control-plane causes (MeetingArn region segment, CreateAttendee idempotency and the reserved 'aws:' prefix, three refusal classes/wordings) and its Evidence section explicitly sets this check aside as a runner-side outbound probe; that is no longer the failure mode — the check now fails on a connection to the address the emulator itself published, so it belongs to the emulator. The two UpdateAttendeeCapabilities / StartMeetingTranscription issues and the BatchCreateAttendee stub issue are unrelated operations.

Cause 1 of 1: Meeting.MediaPlacement.SignalingUrl names a media-signaling endpoint that nothing is listening on, so the advertised data plane cannot be reached at all.
AWS: Smithy models MediaPlacement as "A set of endpoints used by clients to connect to the media service group for an Amazon Chime SDK meeting", with MediaPlacement$SignalingUrl (String, @length 0..4096) documented only as "The signaling URL" — so the liveness requirement is recorded behavior only. The recording shows CreateMeeting answering 200 with a SignalingUrl whose scheme is wss, and that host:port genuinely accepts a TLS connection and completes a TLS handshake; an unauthenticated, non-conforming WebSocket upgrade request issued on its path is then answered at the protocol level with an HTTP/1.1 4xx status line (a refusal of the probe, not a 2xx catch-all and not a dead socket). The endpoint published for a meeting is therefore reachable and speaks the signaling protocol; only the authorization of the upgrade fails.
Emulator: CreateMeeting answers 200 and publishes a SignalingUrl, but no process serves it. Opening a TCP connection to the published host and port (the URL carries no explicit port, so 443, resolved through the harness's endpoint mapping for the published host) fails before any TLS handshake with ConnectionRefusedError: [Errno 111] Connection refused — the emulator never gets to answer an HTTP/1.1 status line at all. The crate says so in its own words: the doc comment above the MediaPlacement derivation in this service's state module states "this emulator runs no Chime SDK media service, so nothing answers behind them", and the SignalingUrl template (unlike the AudioFallbackUrl/ScreenDataUrl/ScreenSharingUrl templates beside it) also carries no port component, so the advertised port defaults to 443 regardless of where the emulator is actually reachable.
Why: a client that does what CreateMeeting's contract invites — take MediaPlacement.SignalingUrl and connect to it to join the meeting's media session — gets a transport-level connection refusal instead of a live endpoint, so the entire meeting data plane is unusable against the emulator even though the control plane reported the meeting created successfully. The failure also arrives as a socket error rather than as any HTTP status or modeled error code, so client code that handles signaling-endpoint rejection (re-fetch the meeting, refresh the join token, back off) never runs, and a client cannot distinguish "not authorized for this signaling session" from "this emulator published an address it does not serve". Nothing about this depends on the concrete host name, region label or meeting id in the URL — any meeting created in any media region is unreachable the same way.
Fix: the MediaPlacement derivation in this service's state module (the media_placement helper that CreateMeeting projects into Meeting.MediaPlacement) together with whatever would have to listen behind it — the published SignalingUrl must name an address the emulator actually serves (host AND port it is reached on, not a bare wss:// host defaulting to 443), and that address must terminate TLS and answer an unauthenticated WebSocket upgrade on the meeting's control path with an HTTP/1.1 4xx status line. File and line citations are under Traces.
Traces: chime-sdk-meetings.native-media-data-plane-requires-client-sdk

## Evidence
Behavioral suite simparity-behavioral-aws-v1, chime-sdk-meetings 15/25 passed (10 failed). Of those 10, one is blocked by the unimplemented BatchCreateAttendee scaffold stub (already filed) and eight are OWNED by open issues; this is the single net-new failure.

Cause 1 — [error] chime-sdk-meetings.native-media-data-plane-requires-client-sdk:
  File "socket.py", line 865, in create_connection
    raise exceptions[0]
  File "socket.py", line 850, in create_connection
    sock.connect(sa)
  ConnectionRefusedError: [Errno 111] Connection refused

The check first calls CreateMeeting (ClientRequestToken, MediaRegion="us-east-1", ExternalMeetingId) — which SUCCEEDS with 200 — then reads Meeting.MediaPlacement.SignalingUrl, asserts ctx.published_url_mismatch(signaling_url, labels=("chime",), websocket=True) reports no mismatch (it does not: the published URL passes that gate), resolves the published host through ctx.endpoint_host(hostname, port or 443), and opens a TLS socket to it.
  Expected: the connection is accepted, the TLS handshake completes, and the server answers the WebSocket upgrade request with an HTTP/1.1 response line whose status is in [400, 500).
  Actual: socket.create_connection() raises ConnectionRefusedError before TLS — nothing is listening on the advertised address, so no status line exists to compare.

Source: services/service-com-amazonaws-chimesdkmeetings-chimemeetingssdkservice/src/state.rs — the media_placement() helper (around lines 597-633) builds signaling_url as "wss://signal.m3.{media_region}.app.chime.aws/control/{meeting_id}" (line ~613-615), with no port component, and the doc comment at lines ~601-603 states "this emulator runs no Chime SDK media service, so nothing answers behind them". services/.../src/ops/create_meeting.rs line ~236 projects that value into Meeting.MediaPlacement ("Server-owned: the media endpoints and the ARN are DERIVED from the minted id"), so every CreateMeeting/GetMeeting read advertises the same unreachable endpoint.

Model: smithy_query(query="shape", shape="MediaPlacement") -> structure, doc "A set of endpoints used by clients to connect to the media service group for an Amazon Chime SDK meeting"; member SignalingUrl -> String @length(0..4096), doc "The signaling URL". The model states no URL format and no liveness rule, hence "recorded behavior only" for the handshake/4xx requirement above.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it (the behavioral suite simparity-behavioral-aws-v1 does this).
2. Point a chime-sdk-meetings client at the emulator and call CreateMeeting with a ClientRequestToken, an ExternalMeetingId and MediaRegion="us-east-1" (any supported media region behaves identically). It answers 200.
3. Read Meeting.MediaPlacement.SignalingUrl from the response. It is a wss:// URL with no port component, so its effective port is 443.
4. Resolve that URL's host to the address the emulator is reachable at and open a TCP/TLS connection to host:443, then send an unauthenticated WebSocket upgrade request (Connection: Upgrade, Upgrade: websocket, Sec-WebSocket-Version: 13, Sec-WebSocket-Key: <any>) on the URL's path.
5. The connect fails with ConnectionRefusedError ([Errno 111]) — no TLS handshake, no HTTP/1.1 status line. AWS accepts the connection, completes the handshake and answers the probe with an HTTP/1.1 4xx.
6. Replay chime-sdk-meetings.native-media-data-plane-requires-client-sdk.
