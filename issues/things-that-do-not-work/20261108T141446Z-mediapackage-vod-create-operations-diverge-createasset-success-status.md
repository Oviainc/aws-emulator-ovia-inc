Issue-Source: trace-sweep-divergence
Issue-Trace: mediapackage-vod-error-paths
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediapackage-vod-error-paths mediapackage-vod-list-assets-filtered-by-packaging-group
Issue-Shape-Id: com.amazonaws.mediapackagevod#MediaPackageVod
Issue-Key: mediapackage-vod|CreatePackagingConfiguration|4xx->4xx|$.message
Issue-Key: mediapackage-vod|CreateAsset|2xx->2xx|status
Issue-Commit: 8755125d33ebbc9bd337be998f0f1ed728c39302

# MediaPackage VOD create operations diverge: CreateAsset success status and CreatePackagingConfiguration validation message

> _Note: this issue was identified against the code base at commit `8755125d33ebbc9bd337be998f0f1ed728c39302`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePackagingConfiguration

## Description
Two causes on the MediaPackage VOD (restJson1, sdkId "MediaPackage Vod") control plane, both on the create path: the UnprocessableEntityException message for a PackagingConfiguration that carries more than one packaging-settings block is worded with an abbreviated resource name, and successful CreateAsset answers the wrong 2xx status. A fixer MUST close BOTH causes below before submitting.

=== Cause 1 of 2: CreatePackagingConfiguration's 422 UnprocessableEntityException message abbreviates "PackagingConfiguration" to "PackagingConfig" ===
AWS: Smithy (services/service-com-amazonaws-mediapackagevod-mediapackagevod/smithy-model/com-amazonaws-mediapackagevod-mediapackagevod.json): CreatePackagingConfiguration POST /packaging_configurations, code 200, input CreatePackagingConfigurationRequest {Id @jsonName("id") @required, PackagingGroupId @jsonName("packagingGroupId") @required, CmafPackage @jsonName("cmafPackage"), DashPackage @jsonName("dashPackage"), HlsPackage @jsonName("hlsPackage"), MssPackage @jsonName("mssPackage"), Tags @jsonName("tags")}; modeled errors ForbiddenException(403), NotFoundException(404), UnprocessableEntityException(422, "The parameters sent in the request are not valid.", member Message @jsonName("message")), TooManyRequestsException(429), InternalServerErrorException(500), ServiceUnavailableException(503). The four package members are mutually exclusive — exactly one of cmafPackage/dashPackage/hlsPackage/mssPackage may be supplied — and the model expresses them as four independent optional members, so the exclusivity is enforced by the handler and surfaced as UnprocessableEntityException. Recorded behavior: a request carrying more than one of those blocks is rejected with HTTP 422, error code UnprocessableEntityException and the message "PackagingConfiguration must include one single type of packaging settings". That whole sentence is FIXED text for this error class — it has no variable parts interpolated from the request or from state (no id, arn, member name, count or limit appears in it), so the only requirement is that the emulator reproduce it verbatim, including the full resource-type name "PackagingConfiguration".
Emulator: returns the correct HTTP status (422) and is matched on every other asserted key, but words the message as "PackagingConfig must include one single type of packaging settings" — the resource type is abbreviated to "PackagingConfig". Replay detail: `$.message: value_mismatch (expected 'PackagingConfiguration must include one single type of packaging settings', got 'PackagingConfig must include one single type of packaging settings')`. No status and no error-code divergence in this cause; only the message text.
Why: the error message is part of the API contract for this error class. A client that surfaces, logs, greps or asserts on the message — a CLI wrapping the SDK error, a CI assertion on a rejected template, a support runbook matching the sentence — sees text AWS never emits, so message-matching error handling and golden-output tests pass against AWS and fail against the emulator (or vice versa). Because the status and code are right, nothing else signals the divergence.
Fix: the validation step in front of the CreatePackagingConfiguration handler in the mediapackagevod service crate — the place that detects multiple packaging-settings blocks and constructs the UnprocessableEntityException message. Emit AWS's wording for that error class with the resource type spelled out in full; the 422 status and the UnprocessableEntityException code are already correct and must not be changed.
Traces: fixture mediapackage-vod-error-paths step task#4 (CreatePackagingConfiguration), verdict mismatch, expected_status 422 / actual_status 422, diverging key $.message.

=== Cause 2 of 2: successful CreateAsset answers HTTP 200 where the recording shows 202 Accepted ===
AWS: recorded behavior, and it CONFLICTS with the on-disk model: Smithy binds CreateAsset to POST /assets with @http code 200 and output CreateAssetResponse {Arn @jsonName("arn"), CreatedAt @jsonName("createdAt") "The time the Asset was initially submitted for Ingest", EgressEndpoints @jsonName("egressEndpoints"), Id @jsonName("id"), PackagingGroupId, ResourceId, SourceArn, SourceRoleArn, Tags}, errors ForbiddenException(403), NotFoundException(404), UnprocessableEntityException(422), TooManyRequestsException(429), InternalServerErrorException(500), ServiceUnavailableException(503). The recorded AWS response for a successful CreateAsset is HTTP 202, which is consistent with the operation's semantics and with the CreatedAt documentation: the call SUBMITS the asset for ingest and the packaging work completes asynchronously (the asset's egress endpoints only become playable later), so the success status is 202 Accepted rather than 200 OK. The corpus asserts the recorded status. This is the same model-vs-recording conflict already noted for DeleteAsset in the related issue below, and the recording is the wire authority.
Emulator: answers HTTP 200 on the successful CreateAsset. Replay detail: `status: expected 202, got 200`. The body keys asserted at this step matched, so this is purely the success status line.
Why: a client that discriminates 202 Accepted (work accepted, result not yet final — keep polling DescribeAsset/EgressEndpoints) from 200 OK (request complete, read the result) takes the wrong branch and treats a freshly submitted asset as finished. Any status assertion on asset creation — SDK integration tests, infrastructure provisioners that gate on 202 before entering a wait loop, HTTP-level clients — fails against the emulator, and the two fixture steps that create assets diverge before the steps that depend on them.
Fix: the CreateAsset handler's success status in the mediapackagevod service crate (and its response-code wiring); reconcile it with the model's @http code 200 in favour of the recorded 202. The response body projection at this step is already correct and must not be changed.
Traces: fixture mediapackage-vod-list-assets-filtered-by-packaging-group step setup#7 (CreateAsset), verdict mismatch, expected_status 202 / actual_status 200, affected_count 2 — the same cluster also covers step setup#8 of that fixture.

Related open issues, none a duplicate: "MediaPackage VOD responses drop required fields: filtered ListAssets body and empty-204 Content-Length" (20261029T174428Z) owns the ListAssets filtering defect and the TagResource/UntagResource Content-Length cases in these same fixtures — it does not touch the CreatePackagingConfiguration validation message or the CreateAsset success status. "DeletePackagingGroup empty 204 response omits the Content-Length: 0 framing header" (20261025T172535Z) is the empty-204 serializer fault only. "MediaPackage VOD: egress manifest URL serves nothing, CreatePackagingGroup is not authorization-gated, and DeleteAsset answers the wrong success status" (20261022T132356Z) is behavioral and covers DeleteAsset's status, the egress data plane and authorization; CreateAsset's success status is a separate handler and is not covered there.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the booted emulator (emulator_up true, bootstrap_signal false, /__admin/health answered 200 and the data listener accepts): 79 steps total, 73 passed, 6 mismatched, 0 errored, 0 blocked, score 92.4%, divergence_count 5. Three of the five clusters are already owned by open issues (DeletePackagingGroup, TagResource, UntagResource Content-Length); the two net-new clusters are the causes below, both service mediapackage-vod, which is bootstrapped (bootstrap gate: pending list empty).

Cause 1 - fixture mediapackage-vod-error-paths, step task#4, operation CreatePackagingConfiguration, service mediapackage-vod, verdict mismatch, affected_count 1
  divergence_key: service=mediapackage-vod|CreatePackagingConfiguration|status=422->422|keys=$.message
  expected: HTTP 422, error UnprocessableEntityException, $.message == "PackagingConfiguration must include one single type of packaging settings"
  actual:   HTTP 422, error UnprocessableEntityException, $.message == "PackagingConfig must include one single type of packaging settings"
  replay detail: $.message: value_mismatch (expected 'PackagingConfiguration must include one single type of packaging settings', got 'PackagingConfig must include one single type of packaging settings')
  The entire sentence is fixed text for this error class; it interpolates nothing from the request or state. Only the message diverges - status and error code match.

Cause 2 - fixture mediapackage-vod-list-assets-filtered-by-packaging-group, step setup#7, operation CreateAsset, service mediapackage-vod, verdict mismatch, affected_count 2 (also setup#8)
  divergence_key: service=mediapackage-vod|CreateAsset|status=202->200|keys=status
  expected: HTTP 202 on successful CreateAsset
  actual:   HTTP 200
  replay detail: status: expected 202, got 200
  note: the on-disk model declares @http code 200 for CreateAsset; the recording shows 202 Accepted. The recording is the wire authority (same conflict pattern as DeleteAsset, where the model says 202 and the recording says 204).

Model citations (smithy_query against services/service-com-amazonaws-mediapackagevod-mediapackagevod/smithy-model/com-amazonaws-mediapackagevod-mediapackagevod.json): CreatePackagingConfiguration http {POST /packaging_configurations, code 200}, input members CmafPackage/DashPackage/HlsPackage/MssPackage all optional, error UnprocessableEntityException {errorKind client, httpError 422, Message @jsonName("message"), doc "The parameters sent in the request are not valid."}; CreateAsset http {POST /assets, code 200}, output CreateAssetResponse {arn, createdAt "The time the Asset was initially submitted for Ingest", egressEndpoints, id, packagingGroupId, resourceId, sourceArn, sourceRoleArn, tags}.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from that checkout.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: look at fixture mediapackage-vod-error-paths, step task#4. By hand, against an existing packaging group: POST /packaging_configurations with a body carrying an id, a packagingGroupId and TWO packaging-settings blocks (e.g. both "hlsPackage" and "dashPackage"), signed with sigv4 credentials the emulator accepts. Observe HTTP 422 with {"message":"PackagingConfig must include one single type of packaging settings"}; AWS returns the same status and code with the message "PackagingConfiguration must include one single type of packaging settings".
4. Cause 2: look at fixture mediapackage-vod-list-assets-filtered-by-packaging-group, steps setup#7 and setup#8. By hand: create an IAM source role, an S3 source bucket and a packaging group, then POST /assets with {id, packagingGroupId, sourceArn, sourceRoleArn} and read the status line with `curl -i`. Observe HTTP 200; AWS's recorded successful CreateAsset answers HTTP 202 Accepted (the body projection is already correct).
5. After fixing, re-replay and confirm all three steps pass: mediapackage-vod-error-paths#task#4, mediapackage-vod-list-assets-filtered-by-packaging-group#setup#7 and mediapackage-vod-list-assets-filtered-by-packaging-group#setup#8.
