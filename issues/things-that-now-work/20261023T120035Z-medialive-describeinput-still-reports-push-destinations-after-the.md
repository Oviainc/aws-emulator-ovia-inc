Issue-Source: trace-sweep-divergence
Issue-Trace: medialive-rtmp-push-input-lifecycle
Issue-Step: teardown#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: medialive-rtmp-push-input-lifecycle
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive|DescribeInput|2xx->2xx|$.destinations[*]
Issue-Commit: 982a07ada62dfbeb289a85399a86f8ade68b0ada

# medialive DescribeInput still reports push destinations after the input is torn down

> _Note: this issue was identified against the code base at commit `982a07ada62dfbeb289a85399a86f8ade68b0ada`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInput

## Description
Cause 1 of 1: DescribeInput returns a destinations entry where AWS returns none, at the teardown read of an RTMP_PUSH input's lifecycle (fixture medialive-rtmp-push-input-lifecycle, step teardown#9).
AWS: Smithy: DescribeInput is GET /prod/inputs/{InputId}, HTTP 200, output member Destinations -> __listOfInputDestination (jsonName 'destinations'); the member is optional and MediaLive stops reporting the allocated push endpoints once the input has left its usable state in the teardown read. Recorded behavior only: the recorded 200 response at this step has no element at $.destinations[0], i.e. an empty/absent destinations list, while every earlier read in the same fixture passed.
Emulator: answers 200 but still carries the originally allocated destination object (shape {ip, port, url, networkRoutes}) in $.destinations[0]. Hypothesis: the emulator keeps the destination list it synthesised at CreateInput on the stored record and serialises it unconditionally, with no state-dependent clearing at teardown/delete.
Why: a client that polls DescribeInput to decide when an input has been released sees endpoints that are no longer live and treats the input as still usable — ingest tooling keeps pointing publishers at a dead rtmp:// URL, and cleanup/verification loops that assert the destinations are gone never finish. The destination values themselves (host, port, generated URL) are environment-specific and are NOT the requirement; the requirement is that destinations is not reported at this point in the lifecycle.
Fix: the DescribeInput response serializer together with the input state record — destinations must be derived from the input's current state rather than echoed verbatim for the record's whole lifetime.
Traces: medialive-rtmp-push-input-lifecycle#teardown#9.

## Evidence
Replay of simparity-wire-aws-v1 (service medialive, verdict=mismatch, expected 200 / actual 200): '$.destinations[0]: extra (expected None, got {\'ip\': \'198.18.0.0\', \'port\': \'1935\', \'url\': \'rtmp://198.18.0.0:1935/...\u2026/live\', \'networkRoutes\': []})' at fixture medialive-rtmp-push-input-lifecycle step teardown#9. The host/port/URL in that object are environment-specific examples; the divergence is the presence of the element at all. Smithy: DescribeInput GET /prod/inputs/{InputId} code 200, output Destinations -> __listOfInputDestination (jsonName 'destinations'), optional.

## Steps to Reproduce
1. CreateInput with type RTMP_PUSH (the emulator allocates a destination with an rtmp:// url).
2. Run the input lifecycle through to its teardown read exactly as fixture medialive-rtmp-push-input-lifecycle does.
3. GET /prod/inputs/<inputId> at the teardown step -> 200 whose 'destinations' array still has the allocated endpoint, where the recording has an empty/absent 'destinations'.
4. Equivalent replay: fixture medialive-rtmp-push-input-lifecycle step teardown#9.
