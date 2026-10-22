Issue-Source: trace-sweep-divergence
Issue-Trace: medialive-rtmp-push-input-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: medialive-rtmp-push-input-lifecycle medialive-url-pull-input-roundtrip
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive|CreateInput|2xx->2xx|$.input.destinations[*].ip
Issue-Key: medialive|CreateInput|2xx->2xx|$.input.destinations[*].networkRoutes
Issue-Key: medialive|CreateInput|2xx->2xx|$.input.destinations[*].port
Issue-Key: medialive|CreateInput|2xx->2xx|$.input.destinations[*].url
Issue-Key: medialive|CreateInput|2xx->2xx|$.input.smpte2110ReceiverGroupSettings
Issue-Key: medialive|DescribeInput|2xx->2xx|$.destinations[*]
Issue-Key: medialive|DescribeInput|2xx->2xx|$.smpte2110ReceiverGroupSettings
Issue-Key: medialive|UpdateInput|2xx->2xx|$.input.destinations[*].ip
Issue-Key: medialive|UpdateInput|2xx->2xx|$.input.destinations[*].networkRoutes
Issue-Key: medialive|UpdateInput|2xx->2xx|$.input.destinations[*].port
Issue-Key: medialive|UpdateInput|2xx->2xx|$.input.destinations[*].url
Issue-Key: medialive|UpdateInput|2xx->2xx|$.input.smpte2110ReceiverGroupSettings
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# medialive Input responses omit destinations and smpte2110ReceiverGroupSettings

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateInput

## Description
Every medialive Input response drops the `smpte2110ReceiverGroupSettings` member, and for push inputs it also drops the generated ingest `destinations` entries (url / ip / port / networkRoutes). This bundle has SIX causes, one per diverging operation/fixture; all share the Input projection defect and all must be closed.

Cause 1 of 6: CreateInput for an RTMP_PUSH input (fixture medialive-rtmp-push-input-lifecycle, step task#2).
AWS: Smithy: CreateInput is POST /prod/inputs code 201, output Input; the Input shape carries Destinations (`__listOfInputDestination`) and Smpte2110ReceiverGroupSettings. For a push input AWS allocates the ingest endpoint and returns it, so the recording shows destinations[0] with url 'rtmp://<ip>:1935/<name>/live', ip, port '1935' and networkRoutes [], and smpte2110ReceiverGroupSettings {'smpte2110ReceiverGroups': []}.
Emulator: returns 201 but $.input.smpte2110ReceiverGroupSettings is absent and $.input.destinations[0] has no url, ip, port or networkRoutes.
Why: the ingest URL is the entire point of creating a push input — a client has nowhere to send the stream, and a caller that reads destinations[0].url gets null. The absent smpte2110ReceiverGroupSettings makes a caller distinguishing receiver-group inputs see an unset member where AWS always sends a (possibly empty) structure.
Fix: the CreateInput handler's Input projection — allocate and return the push destination (url/ip/port/networkRoutes) and always emit smpte2110ReceiverGroupSettings.
Traces: medialive-rtmp-push-input-lifecycle#task#2.

Cause 2 of 6: DescribeInput for an RTMP_PUSH input (fixture medialive-rtmp-push-input-lifecycle, step task#3).
AWS: Smithy: DescribeInput GET /prod/inputs/{InputId} code 200, output includes Destinations and Smpte2110ReceiverGroupSettings; the recording shows the same destination entry and {'smpte2110ReceiverGroups': []}.
Emulator: 200 with smpte2110ReceiverGroupSettings missing and destinations[0] an empty object.
Why: as cause 1 — re-reading the input is how a client recovers the ingest endpoint.
Fix: the DescribeInput Input projection (shared with cause 1).
Traces: medialive-rtmp-push-input-lifecycle#task#3 (also #task#6, #teardown#9).

Cause 3 of 6: UpdateInput for an RTMP_PUSH input (fixture medialive-rtmp-push-input-lifecycle, step task#5).
AWS: the UpdateInput response returns the same Input structure, so destinations[0].url/ip/port/networkRoutes and smpte2110ReceiverGroupSettings are present. HTTP 200.
Emulator: 200 with all five members absent under $.input.
Why: as cause 1; a client that updates an input and relies on the echoed body loses the ingest endpoint.
Fix: the UpdateInput Input projection.
Traces: medialive-rtmp-push-input-lifecycle#task#5.

Cause 4 of 6: CreateInput for a URL_PULL input (fixture medialive-url-pull-input-roundtrip, step task#1).
AWS: smpte2110ReceiverGroupSettings {'smpte2110ReceiverGroups': []} is returned even when no receiver group is configured; HTTP 201.
Emulator: 201 with $.input.smpte2110ReceiverGroupSettings absent.
Why: as cause 1 (destinations are legitimately empty for a pull input, so only the receiver-group member diverges here).
Fix: the CreateInput Input projection.
Traces: medialive-url-pull-input-roundtrip#task#1.

Cause 5 of 6: DescribeInput for a URL_PULL input (fixture medialive-url-pull-input-roundtrip, step task#2).
AWS: 200 with smpte2110ReceiverGroupSettings {'smpte2110ReceiverGroups': []}.
Emulator: 200 with the member absent.
Why: as cause 4.
Fix: the DescribeInput Input projection.
Traces: medialive-url-pull-input-roundtrip#task#2 (also #task#4, #teardown#6).

Cause 6 of 6: UpdateInput for a URL_PULL input (fixture medialive-url-pull-input-roundtrip, step task#3).
AWS: 200 with smpte2110ReceiverGroupSettings {'smpte2110ReceiverGroups': []}.
Emulator: 200 with the member absent.
Why: as cause 4.
Fix: the UpdateInput Input projection.
Traces: medialive-url-pull-input-roundtrip#task#3.

## Evidence
Replay of simparity-wire-aws-v1 (6 clusters, verdict=mismatch, status matched in all):
- CreateInput 201->201 (rtmp push): $.input.smpte2110ReceiverGroupSettings missing (expected {'smpte2110ReceiverGroups': []}); $.input.destinations[0].url missing (expected 'rtmp://184.33.255.41:1935/<name>/live'); .ip missing; .port missing (expected '1935'); .networkRoutes missing (expected []).
- DescribeInput 200->200 (rtmp push, 3 steps): $.smpte2110ReceiverGroupSettings missing; $.destinations[0] missing (expected {url, ip, port '1935', networkRoutes []}, got {}).
- UpdateInput 200->200 (rtmp push): same five members missing under $.input.
- CreateInput 201->201 (url pull): $.input.smpte2110ReceiverGroupSettings missing.
- DescribeInput 200->200 (url pull, 3 steps): $.smpte2110ReceiverGroupSettings missing.
- UpdateInput 200->200 (url pull): $.input.smpte2110ReceiverGroupSettings missing.
Smithy: CreateInput POST /prod/inputs code 201 output Input; DescribeInput output includes Destinations -> __listOfInputDestination and Smpte2110ReceiverGroupSettings. Specific hosts/IPs in the recording are examples, not requirements.

## Steps to Reproduce
1. POST /prod/inputSecurityGroups to get a security group.
2. POST /prod/inputs with type RTMP_PUSH, a destination streamName and that security group.
3. Inspect the 201 body: input.destinations[0] has no url/ip/port/networkRoutes, and input.smpte2110ReceiverGroupSettings is absent.
4. GET /prod/inputs/<id> and PUT /prod/inputs/<id> — same members missing.
5. Repeat with a URL_PULL input: smpte2110ReceiverGroupSettings is absent from create, describe and update.
6. Equivalent replay: fixture medialive-rtmp-push-input-lifecycle step task#2, plus the steps in trace_ids.
