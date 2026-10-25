Issue-Source: trace-sweep-divergence
Issue-Trace: mediapackage-vod-packaging-group-lifecycle
Issue-Step: task#7
Issue-Severity: LOW
Issue-Corpus: simparity-wire-ga-prod-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediapackage-vod-packaging-group-lifecycle
Issue-Shape-Id: com.amazonaws.mediapackagevod#MediaPackageVod
Issue-Key: mediapackage-vod|DeletePackagingGroup|2xx->2xx|$headers.content-length
Issue-Commit: 5ce2202963259763f21ad26fcf06872ee4811807

# DeletePackagingGroup empty 204 response omits the Content-Length: 0 framing header

> _Note: this issue was identified against the code base at commit `5ce2202963259763f21ad26fcf06872ee4811807`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeletePackagingGroup

## Description
One cause on the MediaPackage VOD (restJson1, sdkId "MediaPackage Vod") control plane: the successful, body-less DeletePackagingGroup response is serialized without a Content-Length header, so the response is unframed.

=== Cause 1 of 1: successful DeletePackagingGroup answers with no Content-Length header on its empty body ===
AWS: Smithy: DeletePackagingGroup DELETE /packaging_groups/{Id}, input DeletePackagingGroupRequest {Id @httpLabel @required}, output shape DeletePackagingGroupResponse with NO members (so the success response carries no body), modeled errors ForbiddenException(403), NotFoundException(404), UnprocessableEntityException(422), TooManyRequestsException(429), InternalServerErrorException(500), ServiceUnavailableException(503). The model declares @http code 202, but recorded behavior shows the successful delete answering HTTP 204 and the recording is what the corpus asserts (the emulator already matches on status: expected 204, actual 204 — that part is NOT the divergence). What the recording additionally requires is that this empty-body success response carries the explicit framing header `Content-Length: 0`. The header's value is fixed text for this response class: the output shape has no members, so the serialized body length is always zero regardless of the request. There is no status, error code or error message divergence in this cause.
Emulator: answers the right status (204) and an empty body, but emits NO Content-Length header at all. Replay detail: `$headers.content-length: missing (expected '0', got None)`.
Why: without Content-Length and without Transfer-Encoding, an HTTP/1.1 response body is unframed, so a client must fall back to reading until the connection closes to decide the response is complete. On a keep-alive or pooled connection — which every AWS SDK uses — that is a framing ambiguity: strict clients, proxies and HTTP/1.1 intermediaries in front of the emulator can stall waiting for a body terminator, reject the response as malformed, or poison the pooled connection for the next request on it. Any client or proxy that reads Content-Length to distinguish "no content" from "content not yet framed" takes the wrong branch on an operation whose AWS response is deterministically zero-length.
Fix: the response serializer for DeletePackagingGroup in the mediapackagevod service crate — the path that emits the empty-output success response. It must set Content-Length: 0 on the body-less response (and should do so for every empty-output operation in this service, not just this one handler, since they share the same serialization path). This is a header-emission fix only; the status code and the empty body are already correct and must not be changed.
Traces: fixture mediapackage-vod-packaging-group-lifecycle step task#7 (DeletePackagingGroup), verdict mismatch, expected_status 204 / actual_status 204, diverging key $headers.content-length.

Related open issues, neither a duplicate: "MediaPackage VOD: egress manifest URL serves nothing, CreatePackagingGroup is not authorization-gated, and DeleteAsset answers the wrong success status" (20261022T132356Z) is a behavioral issue covering the egress data plane, request authorization and the DeleteAsset success status — it does not touch DeletePackagingGroup's response headers. "Implement mediapackage-vod TagResource: unimplemented scaffold stub" (20261022T132356Z) is a different operation.

## Evidence
Wire corpus simparity-wire-ga-prod-v1 replayed against the booted emulator: 54 steps, 53 passed, 1 mismatched, 0 errored, 0 blocked (score 98.1%); emulator_up true, bootstrap_signal false.

Cause 1 — fixture mediapackage-vod-packaging-group-lifecycle, step task#7, operation DeletePackagingGroup, service mediapackage-vod, verdict mismatch, affected_count 1
  divergence_key: service=mediapackage-vod|DeletePackagingGroup|status=204->204|keys=$headers.content-length
  expected: HTTP 204 with an empty body AND the response header `content-length: 0`
  actual:   HTTP 204 with an empty body and NO content-length header ($headers.content-length: missing (expected '0', got None))
  status and body match; the sole diverging field is the Content-Length response header. The expected value 0 is not an environment-specific recorded value — DeletePackagingGroupResponse has no members, so the success body is always zero bytes.

Model citations taken from services/service-com-amazonaws-mediapackagevod-mediapackagevod/smithy-model/com-amazonaws-mediapackagevod-mediapackagevod.json (restJson1, sigv4): DeletePackagingGroup http {method DELETE, uri /packaging_groups/{Id}, code 202}, output_shape DeletePackagingGroupResponse (no members).

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from that checkout.
2. Replay the wire corpus simparity-wire-ga-prod-v1 (replay_wire_corpus) and look at fixture mediapackage-vod-packaging-group-lifecycle.
3. The fixture drives the packaging-group lifecycle (create a packaging group, read it back, then delete it). At step task#7 it issues DELETE /packaging_groups/{Id} for the group created earlier in the same fixture.
4. Inspect the raw response headers of that DELETE (e.g. `curl -i -X DELETE http://<emulator-host>:<data-port>/packaging_groups/<id>` against a group that exists, with sigv4 credentials the emulator accepts).
5. Observe: status line 204 and an empty body (both correct), but no `Content-Length` header in the response. AWS's recorded response for the same call carries `content-length: 0`.
6. After fixing, re-replay the fixture and confirm step task#7 passes with the content-length header present and the status still 204.
