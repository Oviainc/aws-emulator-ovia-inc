Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mediapackage-vod.asset-egress-hls-manifest-native-http-round-trip mediapackage-vod.create-packaging-group-missing-permission-rejected mediapackage-vod.delete-asset-state-transition
Issue-Shape-Id: com.amazonaws.mediapackagevod#MediaPackageVod
Issue-Key: mediapackage-vod.asset-egress-hls-manifest-native-http-round-trip|run#17|AssertionError|assert status == 200, f"GET {manifest_url} returned HTTP {status}"
Issue-Key: mediapackage-vod.create-packaging-group-missing-permission-rejected|_error#9|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: mediapackage-vod.delete-asset-state-transition|run#11|AssertionError|assert status_code == 204, f"expected 204 No Content, got {status_code}"
Issue-Commit: b4ac6d961cdb10c212ccdfac1172ee263c3e4aa3

# MediaPackage VOD: egress manifest URL serves nothing, CreatePackagingGroup is not authorization-gated, and DeleteAsset answers the wrong success status

> _Note: this issue was identified against the code base at commit `b4ac6d961cdb10c212ccdfac1172ee263c3e4aa3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePackagingGroup

## Description
Three distinct causes on the MediaPackage VOD (restJson1, sdkId "MediaPackage Vod") surface, all reached by behavioral checks that got past the control-plane calls they needed: the egress endpoint the emulator advertises is not served, CreatePackagingGroup accepts a caller that holds no mediapackage-vod permissions, and DeleteAsset answers a success status other than the recorded one. A fixer must close ALL THREE before submitting.

=== Cause 1 of 3: the egress manifest URL the emulator reports as PLAYABLE returns HTTP 404 (no packaging/egress data plane) ===
AWS: Smithy: DescribeAsset/CreateAsset project EgressEndpoints, a list of EgressEndpoint {PackagingConfigurationId (jsonName packagingConfigurationId), Status (jsonName status, documented as "the current processing status of the asset ... QUEUED, PROCESSING, PLAYABLE, or FAILED"), Url (jsonName url, documented as "The URL of the parent manifest for the repackaged Asset")}. Recorded behavior: once an asset's egress endpoint for an HLS PackagingConfiguration reports Status PLAYABLE, a plain HTTPS GET of that Url returns HTTP 200 with an HLS playlist body whose first non-whitespace token is #EXTM3U — the manifest is actually produced by the packaging pipeline and served on the egress host.
Emulator: the control plane accepts CreatePackagingGroup, CreatePackagingConfiguration and CreateAsset and advertises an egress Url on its egress domain, and the asset's endpoint reaches a status the check accepts as playable, but a GET of that exact advertised Url answers HTTP 404 with no playlist body. The emulator therefore publishes a URL it does not serve.
Why: a client that does what playback does — read EgressEndpoints[].Url and fetch the manifest — gets a 404 instead of a playlist, so nothing can play, and any pipeline that gates on "PLAYABLE means fetchable" advances to a dead URL. The status member becomes a lie about the data plane rather than a report of it.
Fix: the asset/egress data plane of the mediapackagevod service crate — the component that materialises packaged output for an asset+packaging-configuration pair and the HTTP route that serves the advertised egress manifest path, plus the asset state that decides when the endpoint may report a playable status. The URL's host and random path segment are environment-specific and are not requirements; the requirements are that the advertised Url is routable, answers 200, and returns an HLS playlist starting with #EXTM3U.
Traces: mediapackage-vod.asset-egress-hls-manifest-native-http-round-trip (AssertionError: GET <egress-url>/out/v1/<id>/index.m3u8 returned HTTP 404).

=== Cause 2 of 3: CreatePackagingGroup succeeds for a principal with no mediapackage-vod permissions (authorization bypass) ===
AWS: Smithy: CreatePackagingGroup POST /packaging_groups, errors include ForbiddenException (errorKind client, httpError 403, "The client is not authorized to access the requested resource."). Recorded behavior: a caller using credentials from an assumed role with no identity-based policy granting mediapackage-vod actions is rejected with error code AccessDeniedException and no packaging group is created (an independent DescribePackagingGroup on the requested id then reports NotFoundException). The denial message is built from the request and the caller identity — it names the assumed-role principal, the action mediapackage-vod:CreatePackagingGroup and the packaging-groups resource — so that construction rule, not the recorded text, is the requirement.
Emulator: answers HTTP 200 and creates the group for the unprivileged assumed-role credentials (observed response carried Arn arn:aws:mediapackage-vod:<region>:<account>:packaging-groups/<id>, CreatedAt, DomainName, Id, Tags {}), i.e. the request is not authorized at all; the created-anyway group is also a visible side effect of a call AWS refuses.
Why: a client or test that verifies least-privilege — asserting an unauthorized principal is refused, or relying on IAM to contain a mis-scoped role — gets a false pass against the emulator and a resource that AWS would never have created. Error handling keyed on AccessDeniedException never fires.
Fix: the authorization check in front of the CreatePackagingGroup handler in the mediapackagevod service crate (resolve the signing principal's session, evaluate the identity-based policy for mediapackage-vod:CreatePackagingGroup, and reject with AccessDeniedException / 403 before any state write).
Traces: mediapackage-vod.create-packaging-group-missing-permission-rejected (AssertionError: expected the call to be rejected, but it succeeded and returned {...'HTTPStatusCode': 200...}).

=== Cause 3 of 3: DeleteAsset answers HTTP 202 where the recording shows 204 No Content ===
AWS: recorded behavior only, and it CONFLICTS with the on-disk model: Smithy binds DeleteAsset to DELETE /assets/{Id} with @http code 202 and an empty output shape (DeleteAssetResponse), but the recorded AWS response for a successful DeleteAsset is HTTP 204 No Content. The check asserts the recorded status. The asset then resolves to gone asynchronously and a second DeleteAsset on the id fails NotFoundException (errorKind client, httpError 404).
Emulator: answers HTTP 202 on successful DeleteAsset, so the check fails before it can exercise the gone-transition, the re-delete NotFoundException and the id-reuse assertions that follow it — those later assertions are unverified, not known-good.
Why: a client that discriminates 204 No Content (nothing to read) from 202 Accepted takes the wrong branch, and any status assertion on delete fails against the emulator. Because the divergence aborts the check at its first assertion, the rest of the delete lifecycle (eventual NotFoundException from DescribeAsset, NotFoundException on re-delete, id reusable afterwards) is still unconfirmed and should be verified while fixing.
Fix: the DeleteAsset handler's success status in the mediapackagevod service crate; reconcile it with the model's @http code 202 in favour of the recorded 204, and re-run the check to confirm the remaining lifecycle assertions in it.
Traces: mediapackage-vod.delete-asset-state-transition (AssertionError: expected 204 No Content, got 202).

Related open issue, not a duplicate: "MediaPackage VOD success-path projections omit modeled members and lifecycle state is not persisted" (20261022T103701Z) covers success-path projections, ConfigureLogs persistence, ListAssets population and DescribePackagingGroup/DeletePackagingGroup lifecycle on the control plane. It does not cover the egress data plane, request authorization, or the DeleteAsset success status.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 against the booted emulator: mediapackage-vod 5/20 passed, 15 failed. The three causes below are the failures not already filed as unimplemented-operation stubs (DescribePackagingConfiguration, ConfigureLogs, DeletePackagingConfiguration, DeletePackagingGroup, ListPackagingConfigurations, ListPackagingGroups, TagResource, UpdatePackagingGroup are covered by their own filed issues).

Cause 1 — mediapackage-vod.asset-egress-hls-manifest-native-http-round-trip [mismatch]
  expected: HTTP 200 and a body starting with #EXTM3U from the EgressEndpoint Url reported for the HLS packaging configuration once its status is playable
  actual:   AssertionError: GET https://<hex>.egress.mediapackage-vod.us-west-2.amazonaws.com/out/v1/<hex>/index.m3u8 returned HTTP 404
  (host and path ids are environment-specific examples; the requirement is that the advertised Url is served with 200 and an HLS playlist)

Cause 2 — mediapackage-vod.create-packaging-group-missing-permission-rejected [mismatch]
  expected: ClientError with Error.Code == "AccessDeniedException" for CreatePackagingGroup issued with credentials from an assumed role carrying no mediapackage-vod permissions, and the group absent afterwards
  actual:   AssertionError: expected the call to be rejected, but it succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'Arn': 'arn:aws:mediapackage-vod:us-west-2:000000000000:packaging-groups/sim2real-mpvod-deniedgroup-<suffix>', 'CreatedAt': '...', 'DomainName': '<hex>.egress.mediapackage-vod.us-west-2.amazonaws.com', 'Id': 'sim2real-mpvod-deniedgroup-<suffix>', 'Tags': {}}
  (ids, arns, hostnames and timestamps are examples; the requirement is the 403/AccessDeniedException rejection and the absence of any created group)

Cause 3 — mediapackage-vod.delete-asset-state-transition [mismatch]
  expected: DeleteAsset -> ResponseMetadata.HTTPStatusCode == 204
  actual:   AssertionError: expected 204 No Content, got 202
  note: the on-disk model declares @http code 202 for DeleteAsset; the recording shows 204. The check's later assertions (DescribeAsset eventually NotFoundException, re-delete NotFoundException, id reusable) were never reached.

Model citations taken from services/service-com-amazonaws-mediapackagevod-mediapackagevod/smithy-model/com-amazonaws-mediapackagevod-mediapackagevod.json (restJson1, sigv4).

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from the checkout.
2. Replay the behavioral suite simparity-behavioral-aws-v1 (run_behavioral_suite) and watch the three mediapackage-vod checks named in trace_ids.
3. Cause 1: run check mediapackage-vod.asset-egress-hls-manifest-native-http-round-trip (requires ffmpeg on the runner). It creates a packaging group, an HLS packaging configuration, an S3 .smil source plus source role, CreateAsset, polls the asset's EgressEndpoints until the HLS endpoint reports playable, then does a plain HTTPS GET of that endpoint's Url: observe HTTP 404 instead of 200 with an #EXTM3U playlist.
4. Cause 2: run check mediapackage-vod.create-packaging-group-missing-permission-rejected. It creates an IAM role trusted by the account root with NO attached permissions, assumes it via STS, and calls CreatePackagingGroup with those credentials: observe HTTP 200 and a created group instead of AccessDeniedException; then DescribePackagingGroup on that id to see the group AWS would never have created.
5. Cause 3: run check mediapackage-vod.delete-asset-state-transition. Create a packaging group, source bucket/role and asset, then call DeleteAsset and read ResponseMetadata.HTTPStatusCode: observe 202 instead of 204. After fixing the status, let the check continue and confirm DescribeAsset eventually raises NotFoundException, a second DeleteAsset raises NotFoundException, and the asset id is reusable.
