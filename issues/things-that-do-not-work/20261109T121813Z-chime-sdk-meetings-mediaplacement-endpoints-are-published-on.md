Issue-Source: trace-sweep-divergence
Issue-Trace: chime-sdk-meetings-get-meeting-reflects-media-region-and-features
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chime-sdk-meetings-get-meeting-reflects-media-region-and-features
Issue-Shape-Id: com.amazonaws.chimesdkmeetings#ChimeMeetingsSDKService
Issue-Key: chime-sdk-meetings|CreateMeeting|2xx->2xx|$.Meeting.MediaPlacement.AudioFallbackUrl
Issue-Key: chime-sdk-meetings|CreateMeeting|2xx->2xx|$.Meeting.MediaPlacement.EventIngestionUrl
Issue-Key: chime-sdk-meetings|CreateMeeting|2xx->2xx|$.Meeting.MediaPlacement.ScreenDataUrl
Issue-Key: chime-sdk-meetings|CreateMeeting|2xx->2xx|$.Meeting.MediaPlacement.ScreenSharingUrl
Issue-Key: chime-sdk-meetings|CreateMeeting|2xx->2xx|$.Meeting.MediaPlacement.ScreenViewingUrl
Issue-Key: chime-sdk-meetings|CreateMeeting|2xx->2xx|$.Meeting.MediaPlacement.SignalingUrl
Issue-Key: chime-sdk-meetings|CreateMeeting|2xx->2xx|$.Meeting.MediaPlacement.TurnControlUrl
Issue-Key: chime-sdk-meetings|GetMeeting|2xx->2xx|$.Meeting.MediaPlacement.AudioFallbackUrl
Issue-Key: chime-sdk-meetings|GetMeeting|2xx->2xx|$.Meeting.MediaPlacement.EventIngestionUrl
Issue-Key: chime-sdk-meetings|GetMeeting|2xx->2xx|$.Meeting.MediaPlacement.ScreenDataUrl
Issue-Key: chime-sdk-meetings|GetMeeting|2xx->2xx|$.Meeting.MediaPlacement.ScreenSharingUrl
Issue-Key: chime-sdk-meetings|GetMeeting|2xx->2xx|$.Meeting.MediaPlacement.ScreenViewingUrl
Issue-Key: chime-sdk-meetings|GetMeeting|2xx->2xx|$.Meeting.MediaPlacement.SignalingUrl
Issue-Key: chime-sdk-meetings|GetMeeting|2xx->2xx|$.Meeting.MediaPlacement.TurnControlUrl
Issue-Commit: 9cecc173ac9c5f86564428d1030fb1c1f0593211

# chime-sdk-meetings: MediaPlacement endpoints are published on real AWS production hosts the emulator does not serve

> _Note: this issue was identified against the code base at commit `9cecc173ac9c5f86564428d1030fb1c1f0593211`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMeeting

## Description
This issue carries 2 CAUSES, both in the chime-sdk-meetings emulator (crate services/service-com-amazonaws-chimesdkmeetings-chimemeetingssdkservice). Both are wire divergences replayed from simparity-wire-aws-v1; the replay marked neither as owned by any open issue and neither is blocked by a stub.

Relation to already-open chime-sdk-meetings issues: 20261109T115652Z-chime-sdk-meetings-createmeeting-publishes-mediaplacement-signalingurl-on.md is the closest — it is a BEHAVIORAL finding about MediaPlacement.SignalingUrl alone (its cause is stated as SignalingUrl, with the sibling members mentioned only in passing under Fix), and the replay did not report these wire steps as owned by it, so its replay pointer does not cover them. 20261102T093423Z-chime-sdk-meetings-the-signalingurl-createmeeting-advertises-in.md is about the advertised signaling endpoint not being SERVED (a connection refusal behind the address), not about WHICH host is advertised; fixing liveness without changing the host leaves these steps diverging, and vice versa. The remaining open chime-sdk-meetings issues (the 6-net-new-root-causes bundle, the UpdateAttendeeCapabilities and StartMeetingTranscription message issues, the BatchCreateAttendee stub) concern unrelated operations and members. These two causes must be closed by re-replaying the fixture steps named under Traces.

Cause 1 of 2: CreateMeeting's response projects Meeting.MediaPlacement from hard-coded real-AWS host templates, so seven endpoint members announce addresses other than the one the emulator is reached on.
AWS: Smithy: MediaPlacement is documented as "A set of endpoints used by clients to connect to the media service group for an Amazon Chime SDK meeting"; members AudioFallbackUrl, SignalingUrl, TurnControlUrl, ScreenDataUrl, ScreenViewingUrl, ScreenSharingUrl and EventIngestionUrl are each String @length(0..4096) with one-line docs ("The signaling URL.", "The audio fallback URL.", ...). The model states no host format and no liveness rule, so this is recorded behavior only: CreateMeeting answers 200 and the MediaPlacement it returns names endpoints of the deployment that minted the meeting — hosts reachable from the caller, on the port the caller reached the control plane on. The requirement is that RELATION, not any literal host: for an emulator the deployment is the emulator itself, so each member must be ws(s)://<a host the emulator serves>:<the port it is reached on>/<the meeting's path for that endpoint> (or https:// for the two HTTP members), equivalently a per-resource listener on one of the emulator's own hosts. No concrete host, region label or meeting id is part of the requirement.
Emulator: CreateMeeting answers 200 (status matches) but every URL member carries a host in Amazon's live chime.aws domain that this emulator does not and cannot serve, formatted from a fixed production template: AudioFallbackUrl wss://haxrp.m3.<region>.app.chime.aws:443/calls/<meeting-id>, SignalingUrl wss://signal.m3.<region>.app.chime.aws/control/<meeting-id> (no port component at all, so the effective port is 443 regardless of the emulator's port), TurnControlUrl https://ccp.cp.<region>.app.chime.aws/v2/turn_sessions, ScreenDataUrl and ScreenSharingUrl wss://bitpw.m3.<region>.app.chime.aws:443/v2/screen/<meeting-id>, ScreenViewingUrl wss://bitpw.m3.<region>.app.chime.aws:443/ws/connect?passcode=null&viewer_uuid=null&X-BitHub-Call-Id=<meeting-id>, EventIngestionUrl https://data.svc.<region>.ingest.chime.aws/v1/client-events. The replay reports each of the seven as host_not_own: the step's expectation on the value is '<shaped-not-asserted>' (the string's shape is not asserted) while its host must be the emulator's own, and it is not. The crate records the defect in its own comment above the builder: "The shapes of the host names mirror the ones the real service hands out ... this emulator runs no Chime SDK media service, so nothing answers behind them." The region label and meeting GUID in the quoted values are per-run; the defect is the foreign host and, for SignalingUrl, the missing port.
Why: a client that does what the MediaPlacement contract invites — read these members and open the media, signaling, screen or event-ingestion session to actually join the meeting — is sent off the emulator entirely, to real AWS production hostnames, with emulator-minted credentials and a meeting id that does not exist there. In a sandboxed or CI network that is a DNS/egress failure; with egress it is traffic to Amazon's live service. Either way the meeting's data plane is unusable although the control plane reported 200, and the failure surfaces as a name-resolution or transport error rather than any HTTP status or modeled error code, so client paths that handle an endpoint refusal (re-fetch the meeting, refresh the join token, back off) never run.
Fix: the MediaPlacement derivation in this service's state module — the media_placement() helper that CreateMeeting projects into Meeting.MediaPlacement — must build all seven URL members from the emulator's own advertised host and the port it is reached on, plus each endpoint's per-meeting path, instead of from hard-coded *.app.chime.aws / *.ingest.chime.aws templates; the sibling AudioHostUrl host:port token beside them is minted the same foreign way and must move with them so one meeting's endpoints stay mutually consistent. File and line citations are under Traces.
Traces: chime-sdk-meetings-get-meeting-reflects-media-region-and-features#task#1 (operation CreateMeeting, expected 200, actual 200, seven host_not_own key divergences under $.Meeting.MediaPlacement.*).

Cause 2 of 2: GetMeeting re-publishes the same seven foreign MediaPlacement endpoints on every read of the meeting.
AWS: Smithy: GetMeeting returns the same Meeting shape, whose MediaPlacement member is the structure cited in Cause 1 (members String @length(0..4096)); recorded behavior only, as above. A read of a meeting reports the endpoints of the deployment holding that meeting, so GetMeeting's MediaPlacement must satisfy the same own-host relation as the create's — and must equal what CreateMeeting returned for the same MeetingId.
Emulator: GetMeeting answers 200 and returns MediaPlacement with the identical real-AWS hosts listed in Cause 1 (same seven members flagged host_not_own against '<shaped-not-asserted>'), because the read projects through the same media_placement() helper. So the divergence is not confined to the create path: it is re-served on every subsequent read, which is a second replayed step and a second serializer/projection site to change.
Why: a client that stores the MeetingId and re-reads the meeting later to recover its endpoints (the normal rejoin and reconnect path) gets foreign addresses again, so there is no code path by which a client can ever obtain a usable media endpoint from this emulator. Keeping the two paths consistent also matters in the other direction: if only CreateMeeting is fixed, GetMeeting would contradict the create for the same meeting.
Fix: the GetMeeting handler's Meeting projection in this service — which must consume the same corrected media_placement() derivation as CreateMeeting rather than re-deriving or caching the production templates — so one meeting reports one, own-host, mutually consistent set of endpoints from both operations. File and line citations are under Traces.
Traces: chime-sdk-meetings-get-meeting-reflects-media-region-and-features#task#2 (operation GetMeeting, expected 200, actual 200, the same seven host_not_own key divergences under $.Meeting.MediaPlacement.*).

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator built from this checkout: emulator_up=true, bootstrap_signal=false, 16 steps, 14 passed, 2 mismatched, score 87.5%. Both mismatches are the two causes below; stub_path and owned_by are empty for both, so neither is already filed.

Cause 1 — [mismatch] fixture chime-sdk-meetings-get-meeting-reflects-media-region-and-features, step task#1, service chime-sdk-meetings, operation CreateMeeting.
  divergence_key: service=chime-sdk-meetings|CreateMeeting|status=200->200|keys=$.Meeting.MediaPlacement.AudioFallbackUrl,$.Meeting.MediaPlacement.EventIngestionUrl,$.Meeting.MediaPlacement.ScreenDataUrl,$.Meeting.MediaPlacement.ScreenSharingUrl,$.Meeting.MediaPlacement.ScreenViewingUrl,$.Meeting.MediaPlacement.SignalingUrl,$.Meeting.MediaPlacement.TurnControlUrl
  Expected status 200, actual status 200 (the status is NOT the divergence). Per-key verdicts, all host_not_own, expected '<shaped-not-asserted>':
    $.Meeting.MediaPlacement.AudioFallbackUrl  got 'wss://haxrp.m3.us-west-2.app.chime.aws:443/calls/40f2a728-3308-4c61-bb37-5d36052118e8'
    $.Meeting.MediaPlacement.SignalingUrl      got 'wss://signal.m3.us-west-2.app.chime.aws/control/40f2a728-3308-4c61-bb37-5d36052118e8'
    $.Meeting.MediaPlacement.TurnControlUrl    got 'https://ccp.cp.us-west-2.app.chime.aws/v2/turn_sessions'
    $.Meeting.MediaPlacement.ScreenDataUrl     got 'wss://bitpw.m3.us-west-2.app.chime.aws:443/v2/screen/40f2a728-3308-4c61-bb37-5d36052118e8'
    $.Meeting.MediaPlacement.ScreenViewingUrl  got 'wss://bitpw.m3.us-west-2.app.chime.aws:443/ws/connect?passcode=null&viewer_uuid=null&X-BitHub-Call-Id=40f2a728-3308-4c61-bb37-5d36052118e8'
    $.Meeting.MediaPlacement.ScreenSharingUrl  got 'wss://bitpw.m3.us-west-2.app.chime.aws:443/v2/screen/40f2a728-3308-4c61-bb37-5d36052118e8'
    $.Meeting.MediaPlacement.EventIngestionUrl got 'https://data.svc.us-west-2.ingest.chime.aws/v1/client-events'
  The region label us-west-2 and the meeting GUID 40f2a728-... in those values are per-run; the requirement is the relation — the host is one the emulator serves, carrying the port it is reached on.
  Expected: each member's host is one of the emulator's own hosts, with the port the emulator is reached on.
  Actual: hosts in the real AWS domains *.app.chime.aws and *.ingest.chime.aws; SignalingUrl additionally carries no port, so its effective port is 443.

Cause 2 — [mismatch] same fixture, step task#2, service chime-sdk-meetings, operation GetMeeting.
  divergence_key: service=chime-sdk-meetings|GetMeeting|status=200->200|keys=<the same seven $.Meeting.MediaPlacement.* keys>
  Expected status 200, actual status 200; the same seven members come back host_not_own with byte-identical values to Cause 1, confirming the read path re-serves the create path's foreign endpoints rather than diverging independently.

Source: services/service-com-amazonaws-chimesdkmeetings-chimemeetingssdkservice/src/state.rs, media_placement() at line 637 — audio_host_url line 639-642 ('{token}.k.m3.{media_region}.app.chime.aws:3478'), audio_fallback_url 643-645, signaling_url 646-648 (no port component), turn_control_url 649-651, screen_data_url 652-654, screen_sharing_url 655-657, screen_viewing_url 663-666, event_ingestion_url 667-669 — every one a hard-coded *.app.chime.aws / *.ingest.chime.aws template. The doc comment at lines 630-636 states the intent and the consequence: "The shapes of the host names mirror the ones the real service hands out, because a client parses these strings; this emulator runs no Chime SDK media service, so nothing answers behind them." Both CreateMeeting and GetMeeting project this one helper into Meeting.MediaPlacement, which is why the two steps carry identical values.

Model: smithy_query(query="shape", shape="MediaPlacement") on services/service-com-amazonaws-chimesdkmeetings-chimemeetingssdkservice/smithy-model/com-amazonaws-chimesdkmeetings-chimemeetingssdkservice.json -> structure, doc "A set of endpoints used by clients to connect to the media service group for an Amazon Chime SDK meeting"; AudioHostUrl/AudioFallbackUrl/SignalingUrl/TurnControlUrl/ScreenDataUrl/ScreenViewingUrl/ScreenSharingUrl/EventIngestionUrl each String @length(0..4096) with one-line docs (TurnControlUrl/ScreenDataUrl/ScreenViewingUrl/ScreenSharingUrl additionally marked deprecated and no longer used by the Amazon Chime SDK). No host format and no liveness rule is modeled, hence "recorded behavior only" above.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it; note the host and port the data listener is reached on (the replay reported /__admin/health 200 and the data listener accepting).
2. Replay fixture chime-sdk-meetings-get-meeting-reflects-media-region-and-features from trace set simparity-wire-aws-v1.
3. Step task#1 calls CreateMeeting (ClientRequestToken, ExternalMeetingId, MediaRegion — the fixture uses us-west-2; any media region behaves identically, the template just interpolates it) and answers 200.
4. Read Meeting.MediaPlacement from that response: AudioFallbackUrl, SignalingUrl, TurnControlUrl, ScreenDataUrl, ScreenViewingUrl, ScreenSharingUrl and EventIngestionUrl all carry hosts under *.app.chime.aws / *.ingest.chime.aws — real AWS production hostnames, not the host from step 1 — and SignalingUrl carries no port at all. Compare each host:port with the emulator's own: they differ, so every one is reported host_not_own.
5. Step task#2 calls GetMeeting with the MeetingId from step 3. It answers 200 and republishes the identical seven foreign endpoints, so the same seven host_not_own divergences are reported again on the read path.
6. To confirm a fix, re-replay BOTH steps: chime-sdk-meetings-get-meeting-reflects-media-region-and-features#task#1 and #task#2. Each member must name a host the emulator serves, on the port it is reached on, and GetMeeting must return exactly what CreateMeeting returned for the same MeetingId.
