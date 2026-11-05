Issue-Source: trace-sweep-divergence
Issue-Trace: groundstation-tag-and-untag-resource-lifecycle
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: groundstation-tag-and-untag-resource-lifecycle
Issue-Shape-Id: com.amazonaws.groundstation#GroundStation
Issue-Key: groundstation|TagResource|2xx->2xx|$headers.content-length
Issue-Key: groundstation|UntagResource|2xx->2xx|$headers.content-length
Issue-Commit: 8f45cbde1fc786a42a670207326215689e25a831

# Groundstation tag mutations omit Content-Length on their empty 204 responses

> _Note: this issue was identified against the code base at commit `8f45cbde1fc786a42a670207326215689e25a831`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Both Ground Station tagging mutations return a body-less success response whose HTTP envelope is incomplete: the status line is right (204) but the response carries no Content-Length header at all, where the recorded AWS response always carries one. This is one shared fault in how the groundstation crate serializes an operation with an empty output shape, reproduced on both operations that have one.

Cause 1 of 2: TagResource.
  AWS: TagResource is modeled with an empty output structure (Smithy: TagResource -> TagResourceResponse with no members; http POST /tags/{resourceArn}). The recording shows the success response as HTTP 204 with a Content-Length header present (its value is shaped, not asserted, so only the header's presence is a requirement, not any particular number). No error code or message is involved: this is a success path.
  Emulator: answers 204 for the same request but emits no Content-Length header — replay reports '$headers.content-length: missing (expected <shaped-not-asserted>, got None)'.
  Why: a client that frames the response by its declared length, or that validates the response envelope before handing it to the SDK (proxies, HTTP/1.1 intermediaries, strict parsers, and anything that logs or meters response size), sees an unframed response where AWS sends a fully framed one. Against AWS the tag call completes deterministically; against the emulator the same call can be treated as malformed or as a streamed/unknown-length body.
  Fix: the groundstation TagResource handler's response serializer (the empty-output path that builds the 204) must set Content-Length on the response it returns, the same way the recorded service does.
  Traces: groundstation-tag-and-untag-resource-lifecycle#task#3.

Cause 2 of 2: UntagResource.
  AWS: UntagResource is likewise modeled with an empty output structure (Smithy: UntagResource -> UntagResourceResponse with no members; http DELETE /tags/{resourceArn}, idempotent). The recorded success response is HTTP 204 with a Content-Length header present.
  Emulator: answers 204 with no Content-Length header — '$headers.content-length: missing (expected <shaped-not-asserted>, got None)'.
  Why: same client-visible consequence as cause 1, on the untag half of the lifecycle, so neither direction of a tag edit produces an AWS-shaped envelope.
  Fix: the groundstation UntagResource handler's response serializer (same empty-output 204 path) must set Content-Length.
  Traces: groundstation-tag-and-untag-resource-lifecycle#task#5.

No open groundstation issue covers this: the existing open issues concern DescribeContact ephemeris reporting, CreateDataflowEndpointGroupV2 address validation, ListContactVersions lastUpdated, uuid minting, CreateEphemeris DependencyException, ListEphemerides windowing, UpdateConfig cross-group repointing, invented refusal message text, and DeleteDataflowEndpointGroup's not-found message — all of them error/behaviour findings on other operations, none about the success-response envelope of the tagging operations.

## Evidence
Replay of fixture groundstation-tag-and-untag-resource-lifecycle:
- step task#3 (TagResource): expected status 204 WITH a Content-Length header (value shaped, not asserted); actual status 204 with '$headers.content-length: missing (expected <shaped-not-asserted>, got None)'. divergence_key service=groundstation|TagResource|status=204->204|keys=$headers.content-length, verdict mismatch.
- step task#5 (UntagResource): expected status 204 WITH a Content-Length header; actual 204 with '$headers.content-length: missing (expected <shaped-not-asserted>, got None)'. divergence_key service=groundstation|UntagResource|status=204->204|keys=$headers.content-length, verdict mismatch.
Both operations are modeled with empty output shapes (TagResourceResponse / UntagResourceResponse have no members), so the only wire content of a success is the status plus the headers that frame it.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Fixture groundstation-tag-and-untag-resource-lifecycle creates a taggable Ground Station resource, then at step task#3 calls TagResource POST /tags/{resourceArn} with a tags map, and at step task#5 calls UntagResource DELETE /tags/{resourceArn}?tagKeys=... .
3. Inspect the raw HTTP response of each: both return 204 but neither response carries a Content-Length header, while the recorded AWS responses for the same two calls do.
4. After a fix, re-replay groundstation-tag-and-untag-resource-lifecycle#task#3 and #task#5; both must answer 204 with Content-Length present.
