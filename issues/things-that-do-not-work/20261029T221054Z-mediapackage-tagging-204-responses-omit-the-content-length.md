Issue-Source: trace-sweep-divergence
Issue-Trace: mediapackage-channel-lifecycle-tags-logs
Issue-Step: task#6
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediapackage-channel-lifecycle-tags-logs mediapackage-origin-endpoint-lifecycle
Issue-Shape-Id: com.amazonaws.mediapackage#MediaPackage
Issue-Key: mediapackage|TagResource|2xx->2xx|$headers.content-length
Issue-Key: mediapackage|UntagResource|2xx->2xx|$headers.content-length
Issue-Commit: bcbe4ab72d6cacb413c4017b750730a99b5d228f

# MediaPackage tagging 204 responses omit the Content-Length framing header (TagResource, UntagResource)

> _Note: this issue was identified against the code base at commit `bcbe4ab72d6cacb413c4017b750730a99b5d228f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Two causes on the MediaPackage (restJson1, sdkId "MediaPackage") control plane, sharing ONE root cause: the empty-output success responses of the tagging operations are serialized without a Content-Length header, so a body-less 204 goes out unframed. Both causes must be closed before submitting.

=== Cause 1 of 2: successful TagResource answers 204 with no Content-Length header ===
AWS: Smithy (services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json): TagResource http {method POST, uri /tags/{ResourceArn}, code 204}, input TagResourceRequest {ResourceArn @httpLabel @required, Tags (jsonName "tags") @required}, output shape Unit — the success response has NO members and therefore no body. The corpus asserts that this empty-body 204 carries an explicit content-length response header (shaped, value not pinned); because the output shape has no members the serialized length is always zero for this response class, so the header's value is fixed text (0) for every request and is not an environment- or request-specific recorded value. No status, error code or error message diverges in this cause: the status is 204 on both sides.
Emulator: answers the correct status (204) with an empty body, but emits NO content-length header at all. Replay detail: `$headers.content-length: missing (expected '<shaped-not-asserted>', got None)`.
Why: an HTTP/1.1 response with neither Content-Length nor Transfer-Encoding is unframed, so a client must read until connection close to know the response ended. Every AWS SDK uses pooled keep-alive connections, so a strict client, proxy or HTTP/1.1 intermediary in front of the emulator can stall waiting for a body terminator, reject the response as malformed, or poison the pooled connection for the next request on it. A client that reads Content-Length to distinguish "no content" from "body not yet framed" takes the wrong branch on an operation whose AWS response is deterministically zero-length.
Fix: the response serializer for TagResource in the mediapackage service crate — the path that emits the empty-output (Unit) success response. It must set Content-Length: 0 on the body-less response; because this service's empty-output operations share that serialization path, fix it there rather than in the single handler. Header emission only: the 204 status and the empty body are already correct and must not change.
Traces: fixture mediapackage-channel-lifecycle-tags-logs step task#6 (TagResource), verdict mismatch, expected_status 204 / actual_status 204, diverging key $headers.content-length; same cluster also affects mediapackage-origin-endpoint-lifecycle step task#7.

=== Cause 2 of 2: successful UntagResource answers 204 with no Content-Length header ===
AWS: Smithy (same model): UntagResource http {method DELETE, uri /tags/{ResourceArn}, code 204}, input UntagResourceRequest {ResourceArn @httpLabel @required, TagKeys @httpQuery "tagKeys" @required}, output shape Unit — no members, so no response body. The recording requires the empty-body 204 to carry a content-length header; its value is fixed text (0) for this response class, independent of the ResourceArn or the tag keys removed. No status, error code or error message divergence: 204 on both sides.
Emulator: answers 204 with an empty body but emits NO content-length header. Replay detail: `$headers.content-length: missing (expected '<shaped-not-asserted>', got None)`.
Why: same framing ambiguity as cause 1 — an unframed body-less response on a pooled keep-alive connection is read as "incomplete" rather than "empty" by strict clients and intermediaries, which can hang the request, surface a protocol error for a call AWS answers cleanly, or corrupt the next request on that connection.
Fix: the same empty-output response-serialization path in the mediapackage service crate, reached from the UntagResource handler. Emit Content-Length: 0 for every Unit-output operation of this service; do not change the status or the empty body.
Traces: fixture mediapackage-channel-lifecycle-tags-logs step task#8 (UntagResource), verdict mismatch, expected_status 204 / actual_status 204, diverging key $headers.content-length; same cluster also affects mediapackage-origin-endpoint-lifecycle step task#9.

Related open issues, none a duplicate: "DeletePackagingGroup empty 204 response omits the Content-Length: 0 framing header" (20261025T172535Z) and "MediaPackage VOD responses drop required fields: filtered ListAssets body and empty-204 Content-Length" (20261029T174428Z) are the same class of defect but in the separate mediapackage-vod service crate, so neither covers these mediapackage operations. "TagResource judges the tag-content rules only after resolving the ARN, so a request UntagResource and both creates answer 422 is answered 404" (20261029T014446Z) and "UntagResource skips the tag-key length bound its sibling TagResource and both creates apply to the same keys" (20261029T014446Z) concern these two operations' validation ordering and tag-key bounds on FAILURE paths, not the headers of their successful empty responses.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the booted emulator: 7 fixtures replayed, 65 steps, 61 passed, 4 mismatched, 0 errored, 0 blocked (score 93.8%); emulator_up true ("/__admin/health answered 200 and the :38269 data listener accepts"), bootstrap_signal false, divergence_count 2. Service token "mediapackage" is bootstrapped per get_bootstrap_status.

Cause 1 — fixture mediapackage-channel-lifecycle-tags-logs, step task#6, operation TagResource, service mediapackage, verdict mismatch, affected_count 2
  divergence_key: service=mediapackage|TagResource|status=204->204|keys=$headers.content-length
  expected: HTTP 204, empty body, AND a content-length response header (shaped; 0 for an empty body)
  actual:   HTTP 204, empty body, NO content-length header — `$headers.content-length: missing (expected '<shaped-not-asserted>', got None)`
  affected: mediapackage-channel-lifecycle-tags-logs#task#6, mediapackage-origin-endpoint-lifecycle#task#7
  stub_path empty, owned_by empty — not blocked by an unimplemented operation and not owned by an existing issue.

Cause 2 — fixture mediapackage-channel-lifecycle-tags-logs, step task#8, operation UntagResource, service mediapackage, verdict mismatch, affected_count 2
  divergence_key: service=mediapackage|UntagResource|status=204->204|keys=$headers.content-length
  expected: HTTP 204, empty body, AND a content-length response header (shaped; 0 for an empty body)
  actual:   HTTP 204, empty body, NO content-length header — `$headers.content-length: missing (expected '<shaped-not-asserted>', got None)`
  affected: mediapackage-channel-lifecycle-tags-logs#task#8, mediapackage-origin-endpoint-lifecycle#task#9
  stub_path empty, owned_by empty.

In both causes the status code and the (empty) body match the recording; the sole diverging field is the Content-Length response header. Model citations: TagResource output_shape Unit, http code 204; UntagResource output_shape Unit, http code 204 — so the expected length is structurally zero and carries no per-run value.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from that checkout.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus) and look at fixtures mediapackage-channel-lifecycle-tags-logs and mediapackage-origin-endpoint-lifecycle.
3. Cause 1: at step task#6 the channel-lifecycle fixture issues POST /tags/{ResourceArn} (TagResource) for the channel it created earlier in the same fixture. Equivalent by hand: `curl -i -X POST http://<emulator-host>:<data-port>/tags/<url-encoded-channel-arn> -d '{"tags":{"env":"test"}}'` with sigv4 credentials the emulator accepts, against a channel that exists.
4. Observe a 204 status line and an empty body (both correct) but no `Content-Length` header in the response; AWS's recorded response carries one (value 0).
5. Cause 2: at step task#8 the same fixture issues DELETE /tags/{ResourceArn}?tagKeys=<key> (UntagResource) for that channel. Equivalent by hand: `curl -i -X DELETE 'http://<emulator-host>:<data-port>/tags/<url-encoded-channel-arn>?tagKeys=env'`.
6. Observe the same omission: 204, empty body, no `Content-Length` header.
7. The same two clusters reproduce on the origin-endpoint path: mediapackage-origin-endpoint-lifecycle step task#7 (TagResource) and step task#9 (UntagResource).
8. After fixing, re-replay both fixtures and confirm steps task#6/task#8 (and task#7/task#9 of mediapackage-origin-endpoint-lifecycle) pass with the content-length header present and the status still 204.
