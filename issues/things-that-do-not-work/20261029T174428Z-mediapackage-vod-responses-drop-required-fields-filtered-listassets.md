Issue-Source: trace-sweep-divergence
Issue-Trace: mediapackage-vod-list-assets-filtered-by-packaging-group
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediapackage-vod-list-assets-filtered-by-packaging-group mediapackage-vod-tagging-round-trip
Issue-Shape-Id: com.amazonaws.mediapackagevod#MediaPackageVod
Issue-Key: mediapackage-vod|ListAssets|2xx->2xx|$.assets[*]
Issue-Key: mediapackage-vod|TagResource|2xx->2xx|$headers.content-length
Issue-Key: mediapackage-vod|UntagResource|2xx->2xx|$headers.content-length
Issue-Commit: c5b0e480c5d08ce7c34e019fd56b1aa2c0c8407c

# MediaPackage VOD responses drop required fields: filtered ListAssets body and empty-204 Content-Length

> _Note: this issue was identified against the code base at commit `c5b0e480c5d08ce7c34e019fd56b1aa2c0c8407c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAssets

## Description
Three causes on the MediaPackage VOD (restJson1, sdkId "MediaPackage Vod") control plane. All three answer the correct HTTP status but serialize a response that is missing a field the wire requires: the ListAssets success body omits the assets the request asked for, and two body-less 204 successes omit their framing header. A fixer MUST close all three causes below before submitting.

=== Cause 1 of 3: ListAssets filtered by packagingGroupId returns no assets ===
AWS: Smithy (services/service-com-amazonaws-mediapackagevod-mediapackagevod/smithy-model/com-amazonaws-mediapackagevod-mediapackagevod.json): ListAssets GET /assets, code 200, input ListAssetsRequest {MaxResults @httpQuery("maxResults") @range(min 1,max 1000), NextToken @httpQuery("nextToken"), PackagingGroupId @httpQuery("packagingGroupId")}, output ListAssetsResponse {Assets @jsonName("assets") -> __listOfAssetShallow, NextToken @jsonName("nextToken")}, @paginated(items: Assets). PackagingGroupId is a server-side FILTER, not a free-text echo: when it is supplied, AWS returns every asset in the caller's account/region whose packagingGroupId equals that value, each as an AssetShallow carrying at minimum id, arn (arn:aws:mediapackage-vod:<region>:<account>:assets/<id>), sourceArn, sourceRoleArn, packagingGroupId, createdAt and tags. An asset created against that packaging group earlier in the same session MUST appear in the filtered listing. Modeled errors are ForbiddenException(403), NotFoundException(404), UnprocessableEntityException(422), TooManyRequestsException(429), InternalServerErrorException(500), ServiceUnavailableException(503) — none of them applies to a successful listing, and the recorded response is HTTP 200.
Emulator: answers HTTP 200 (status matches) but with no entry at $.assets[0] — the filtered listing comes back empty even though the fixture created an asset in that packaging group earlier in the same fixture and those creation steps passed. Replay detail: `$.assets[0]: missing (expected {'id': '{{asset_a_id}}', 'arn': 'arn:aws:mediapackage-vod:us-west-2:000000000000:assets/{{asset_a_id}}', 'sourceArn': 'arn:aws:s3:::{{bucket_name}}/source/asset.m3u8', 'sourceRoleArn': ..., 'packagingGroupId': ..., 'createdAt': '<shaped-not-asserted>', 'tags': {}}, got None)`. Only the presence and content of the asset entry diverge; there is no status, error code or error message divergence in this cause.
Why: a client that creates an asset and then lists by packagingGroupId to discover it sees an empty collection, so any workflow that enumerates assets to find, package, poll or delete them concludes the asset does not exist. Paginated SDK iterators terminate immediately, infrastructure tools treat the asset as absent and re-create it, and cleanup paths silently skip it. This is a wrong-data defect, not a cosmetic one: the response is well-formed and successful, so the client has no signal that anything went wrong.
Fix: the ListAssets handler in the mediapackagevod service crate, together with the asset state it reads — either created assets are not recorded in the listable store keyed by packaging group, or the packagingGroupId query filter does not match the stored value (an exact-equality match against the asset's packagingGroupId is what is required), or the handler never projects stored assets into the AssetShallow list members. The filter must be applied over the stored assets and each surviving asset projected with id, arn, sourceArn, sourceRoleArn, packagingGroupId, createdAt and tags; an unfiltered request must return the same assets without the filter applied. The arn, ids and bucket names in the recorded expectation are environment values filled into that shape, not literals to hardcode.
Traces: fixture mediapackage-vod-list-assets-filtered-by-packaging-group step task#9 (ListAssets), verdict mismatch, expected_status 200 / actual_status 200, diverging key $.assets[0].

=== Cause 2 of 3: successful TagResource 204 omits the Content-Length: 0 framing header ===
AWS: Smithy: TagResource POST /tags/{ResourceArn}, code 204, input TagResourceRequest {ResourceArn @httpLabel @required, Tags @jsonName("tags") @required -> map of string}, output shape Unit (no members, so the success response carries no body), no modeled errors. Recorded behavior: the successful call answers HTTP 204 with an empty body AND the explicit framing header `content-length: 0`. That value is fixed text for this response class — the output has no members, so the serialized body is always zero bytes regardless of the request or of how many tags were set.
Emulator: answers the right status (204) with an empty body, but emits NO Content-Length header at all. Replay detail: `$headers.content-length: missing (expected '0', got None)`. No status, error code or error message divergence.
Why: an HTTP/1.1 response with neither Content-Length nor Transfer-Encoding is unframed, so a client must read until the connection closes to decide the response is complete. On the pooled keep-alive connections every AWS SDK uses, that is a framing ambiguity: strict clients and intervening HTTP/1.1 proxies can stall waiting for a terminator, reject the response as malformed, or poison the pooled connection for the next request on it.
Fix: the response serializer for the empty-output success path of TagResource in the mediapackagevod service crate — it must set Content-Length: 0 on the body-less response. The status code and the empty body are already correct and must not be changed.
Traces: fixture mediapackage-vod-tagging-round-trip step task#3 (TagResource), verdict mismatch, expected_status 204 / actual_status 204, diverging key $headers.content-length.

=== Cause 3 of 3: successful UntagResource 204 omits the Content-Length: 0 framing header ===
AWS: Smithy: UntagResource DELETE /tags/{ResourceArn}, code 204, input UntagResourceRequest {ResourceArn @httpLabel @required, TagKeys @httpQuery("tagKeys") @required -> list of string}, output shape Unit (no members, no body), no modeled errors. Recorded behavior: HTTP 204, empty body, with `content-length: 0` present. Again a fixed value for the response class, independent of which tag keys were removed.
Emulator: HTTP 204 with an empty body and no Content-Length header. Replay detail: `$headers.content-length: missing (expected '0', got None)`. No status, error code or error message divergence.
Why: same framing ambiguity as cause 2 — the body-less response cannot be delimited by length, so strict clients, proxies and connection pools in front of the emulator can stall on, reject, or mis-sequence it.
Fix: the same empty-output response serialization path in the mediapackagevod service crate, for UntagResource. Because causes 2 and 3 (and the already-filed DeletePackagingGroup case) share that path, the correct fix is to emit Content-Length: 0 for EVERY empty-output operation of this service rather than patching one handler at a time.
Traces: fixture mediapackage-vod-tagging-round-trip step task#5 (UntagResource), verdict mismatch, expected_status 204 / actual_status 204, diverging key $headers.content-length.

Related open issues, none a duplicate: "DeletePackagingGroup empty 204 response omits the Content-Length: 0 framing header" (20261025T172535Z) is the SAME serializer fault on a third operation, DeletePackagingGroup, and already carries that step's replay key — causes 2 and 3 here are TagResource and UntagResource and are not covered by it, though one serializer change can close all three. "MediaPackage VOD: egress manifest URL serves nothing, CreatePackagingGroup is not authorization-gated, and DeleteAsset answers the wrong success status" (20261022T132356Z) and "Egress endpoint Status is a 500 ms wall-clock timer..." (20261024T071731Z) are behavioral issues about the egress data plane, request authorization and DeleteAsset/DescribeAsset; neither touches ListAssets filtering or tag-response framing.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the booted emulator (emulator_up true, bootstrap_signal false, /__admin/health 200): 79 steps total, 66 passed, 5 mismatched, 0 errored, 8 blocked, score 83.5%, divergence_count 5. The four net-new divergences all belong to bootstrapped services; three of them are the mediapackage-vod causes below.

Cause 1 — fixture mediapackage-vod-list-assets-filtered-by-packaging-group, step task#9, operation ListAssets, service mediapackage-vod, verdict mismatch, affected_count 1
  divergence_key: service=mediapackage-vod|ListAssets|status=200->200|keys=$.assets[0]
  expected: HTTP 200 with assets[0] = the asset created earlier in the fixture against the filtered packaging group, projected as AssetShallow {id, arn arn:aws:mediapackage-vod:<region>:<account>:assets/<id>, sourceArn, sourceRoleArn, packagingGroupId, createdAt (shape asserted, value not), tags {}}
  actual:   HTTP 200 with nothing at $.assets[0] — `$.assets[0]: missing (expected {...}, got None)`
  status matches; the body omits the asset the packagingGroupId filter should have returned. The concrete id/arn/bucket values in the expectation are per-run environment values, not requirements.

Cause 2 — fixture mediapackage-vod-tagging-round-trip, step task#3, operation TagResource, service mediapackage-vod, verdict mismatch, affected_count 1
  divergence_key: service=mediapackage-vod|TagResource|status=204->204|keys=$headers.content-length
  expected: HTTP 204, empty body, response header `content-length: 0`
  actual:   HTTP 204, empty body, NO content-length header (`$headers.content-length: missing (expected '0', got None)`)

Cause 3 — fixture mediapackage-vod-tagging-round-trip, step task#5, operation UntagResource, service mediapackage-vod, verdict mismatch, affected_count 1
  divergence_key: service=mediapackage-vod|UntagResource|status=204->204|keys=$headers.content-length
  expected: HTTP 204, empty body, response header `content-length: 0`
  actual:   HTTP 204, empty body, NO content-length header (`$headers.content-length: missing (expected '0', got None)`)

Model citations (smithy_query against services/service-com-amazonaws-mediapackagevod-mediapackagevod/smithy-model/com-amazonaws-mediapackagevod-mediapackagevod.json): ListAssets http {GET /assets, code 200}, input PackagingGroupId @httpQuery("packagingGroupId"), output ListAssetsResponse {Assets @jsonName("assets") -> __listOfAssetShallow, NextToken}, @paginated(items: Assets); TagResource http {POST /tags/{ResourceArn}, code 204}, output Unit; UntagResource http {DELETE /tags/{ResourceArn}, code 204}, output Unit.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from that checkout.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: look at fixture mediapackage-vod-list-assets-filtered-by-packaging-group. The fixture creates an IAM role, an S3 source bucket, a packaging group and an asset in that group, then at step task#9 issues GET /assets?packagingGroupId=<that group id>. Equivalent by hand: create a packaging group, CreateAsset into it, then `curl -s 'http://<emulator-host>:<data-port>/assets?packagingGroupId=<group-id>'` with sigv4 credentials the emulator accepts. Observe HTTP 200 with an empty/absent assets list; AWS returns the created asset as an AssetShallow entry. Also check the unfiltered GET /assets to see whether the asset is listable at all — that distinguishes a broken filter from missing list state.
4. Causes 2 and 3: look at fixture mediapackage-vod-tagging-round-trip, steps task#3 (TagResource) and task#5 (UntagResource). By hand: `curl -i -X POST 'http://<emulator-host>:<data-port>/tags/<resource-arn>' -d '{"tags":{"k":"v"}}'` and `curl -i -X DELETE 'http://<emulator-host>:<data-port>/tags/<resource-arn>?tagKeys=k'` against an existing MediaPackage VOD resource.
5. Observe in both: status line 204 and an empty body (correct), but no `Content-Length` header. AWS's recorded responses carry `content-length: 0`.
6. After fixing, re-replay and confirm ALL THREE steps pass: mediapackage-vod-list-assets-filtered-by-packaging-group#task#9, mediapackage-vod-tagging-round-trip#task#3 and mediapackage-vod-tagging-round-trip#task#5.
