Issue-Source: trace-sweep-divergence
Issue-Trace: chime-sdk-meetings-get-meeting-reflects-media-region-and-features
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chime-sdk-meetings-get-meeting-reflects-media-region-and-features
Issue-Shape-Id: com.amazonaws.chimesdkmeetings#ChimeMeetingsSDKService
Issue-Key: chime-sdk-meetings|CreateMeeting|2xx->2xx|$.Meeting.MediaPlacement.ScreenViewingUrl
Issue-Key: chime-sdk-meetings|CreateMeeting|2xx->2xx|$.Meeting.TenantIds
Issue-Key: chime-sdk-meetings|GetMeeting|2xx->2xx|$.Meeting.MediaPlacement.ScreenViewingUrl
Issue-Key: chime-sdk-meetings|GetMeeting|2xx->2xx|$.Meeting.TenantIds
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# Meeting projection diverges: ScreenViewingUrl template and absent TenantIds

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMeeting

## Description
This issue bundles 2 CAUSES, both in the single `Meeting` projection that `CreateMeeting` mints and that every later reader (`GetMeeting`) returns unchanged. Both causes reproduce on BOTH replayed steps of the fixture (task#1 = CreateMeeting, task#2 = GetMeeting), because GetMeeting returns the record CreateMeeting stored; a fixer must close BOTH causes before submitting.

Cause 1 of 2: `Meeting.MediaPlacement.ScreenViewingUrl` is emitted with the wrong URL template.
AWS: recorded behavior only. The Smithy model constrains `MediaPlacement$ScreenViewingUrl` to `String @length(min:0,max:4096)` and documents only "The screen viewing URL" (it is marked deprecated/no longer used by the SDK, but it is still populated on the wire), so the model states no format and the requirement comes from the recording. The recorded wire value for this member is the Chime bit-hub websocket connect URL carrying the viewer query string, i.e. the fixed text `wss://bitpw.m3.` + `.app.chime.aws:443/ws/connect?passcode=null&viewer_uuid=null&X-BitHub-Call-Id=` with the meeting's `MediaRegion` filling the host's region label and the minted `MeetingId` filling the call-id parameter. The `passcode` and `viewer_uuid` query parameters are part of that fixed text, not per-run values. Status is 200 in both the recording and the emulator; there is no error code involved.
Emulator: serves 200 with `$.Meeting.MediaPlacement.ScreenViewingUrl` built WITHOUT the `passcode`/`viewer_uuid` parameters — only `...app.chime.aws:443/ws/connect?X-BitHub-Call-Id=<MeetingId>` — so the replay reports `$.Meeting.MediaPlacement.ScreenViewingUrl: value_mismatch (expected '<shaped-not-asserted>', got 'wss://bitpw.m3.<region>.app.chime.aws:443/ws/connect?X-BitHub-Call-Id=<meeting-guid>')`. (The concrete host, region label and GUID in that string are per-run values and are NOT the requirement; the missing query parameters are.)
Why: a client that parses the screen-viewing URL, or that round-trips it into a legacy screen-viewing component, loses the `passcode`/`viewer_uuid` parameters the real service always supplies, so URL parsing and any assertion on the connect query string fails against the emulator while succeeding against AWS.
Fix: the shared `MediaPlacement` derivation that both the create handler and the readers project from — the `screen_viewing_url` template in this service's state module (see Traces for the file/line citation).

Cause 2 of 2: `Meeting.TenantIds` is omitted from the response instead of being serialized as an empty list when the create named no tenant ids.
AWS: recorded behavior only. The model's `Meeting$TenantIds` targets `TenantIdList @length(min:1,max:5)`, which constrains a NON-empty list when one is supplied but says nothing about the member's presence; the recording shows the member always present on the `Meeting` projection, serialized as `[]` when the creating request carried no `TenantIds`. Status 200, no error code.
Emulator: serves 200 with the member entirely absent — the create handler passes the request's optional `TenantIds` straight through, so an absent input yields an absent output member. The replay reports `$.Meeting.TenantIds: missing (expected [], got None)`.
Why: a client that reads `Meeting.TenantIds` unconditionally (iterating it, or treating its presence as proof the projection is complete) gets a null/absent member from the emulator where AWS always hands back a list, so tenant-scoping logic either throws on a null or takes a different branch than it does in production.
Fix: the `CreateMeeting` handler's `Meeting` construction (and therefore the stored record every reader projects), which must default the member to an empty list rather than propagating `None`; alternatively the output serializer for the `Meeting` shape if presence is normalised there.

No open issue covers either cause (no existing chime issues are filed).

## Evidence
Replay of fixture `chime-sdk-meetings-get-meeting-reflects-media-region-and-features` (trace set simparity-wire-aws-v1), 2 divergence clusters, both verdict=mismatch, expected_status 200 / actual_status 200:

Step task#1 (CreateMeeting), divergence_key `service=chime-sdk-meetings|CreateMeeting|status=200->200|keys=$.Meeting.MediaPlacement.ScreenViewingUrl,$.Meeting.TenantIds`:
  $.Meeting.MediaPlacement.ScreenViewingUrl: value_mismatch (expected '<shaped-not-asserted>', got 'wss://bitpw.m3.us-west-2.app.chime.aws:443/ws/connect?X-BitHub-Call-Id=e2e1bfbf-08f3-4a77-bf49-45dd08e57749')
  $.Meeting.TenantIds: missing (expected [], got None)

Step task#2 (GetMeeting), divergence_key `service=chime-sdk-meetings|GetMeeting|status=200->200|keys=$.Meeting.MediaPlacement.ScreenViewingUrl,$.Meeting.TenantIds`:
  identical two key divergences, because GetMeeting returns the stored record CreateMeeting minted.

Expected (AWS, per recording):
  ScreenViewingUrl = wss://bitpw.m3.<MediaRegion>.app.chime.aws:443/ws/connect?passcode=null&viewer_uuid=null&X-BitHub-Call-Id=<MeetingId>
  TenantIds = [] when the create supplied none
Actual (emulator):
  ScreenViewingUrl = wss://bitpw.m3.<MediaRegion>.app.chime.aws:443/ws/connect?X-BitHub-Call-Id=<MeetingId>   (passcode/viewer_uuid parameters absent)
  TenantIds absent from the Meeting object

Model references used: smithy_query(query="operation", operation="CreateMeeting,GetMeeting") — both return shape `Meeting`, http 200; smithy_query(query="shape", shape="Meeting,MediaPlacement,TenantIdList") — `MediaPlacement$ScreenViewingUrl` is `String @length(0..4096)` with no format trait, `Meeting$TenantIds` targets `TenantIdList @length(1..5)`.

Source citations: services/service-com-amazonaws-chimesdkmeetings-chimemeetingssdkservice/src/state.rs lines 600-602 (`screen_viewing_url` template, no passcode/viewer_uuid parameters); services/service-com-amazonaws-chimesdkmeetings-chimemeetingssdkservice/src/ops/create_meeting.rs line 252 (`tenant_ids: input.tenant_ids`, propagating the absent input member).

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay fixture `chime-sdk-meetings-get-meeting-reflects-media-region-and-features` from trace set simparity-wire-aws-v1 (`replay_wire_corpus`).
3. Step task#1 POSTs /meetings (CreateMeeting) with a ClientRequestToken, ExternalMeetingId, MediaRegion and MeetingFeatures and no TenantIds; step task#2 GETs /meetings/{MeetingId} for the returned id.
4. Both steps answer 200. Inspect the response body of each:
   - `$.Meeting.MediaPlacement.ScreenViewingUrl` lacks the `passcode=null&viewer_uuid=null` query parameters the recorded AWS response carries before `X-BitHub-Call-Id`.
   - `$.Meeting.TenantIds` is absent, where the recorded AWS response carries `[]`.
5. Both key divergences are reported for task#1 and again for task#2.
