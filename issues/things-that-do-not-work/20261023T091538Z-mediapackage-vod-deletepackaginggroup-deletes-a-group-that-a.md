Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mediapackage-vod.delete-packaging-group-with-dependent-configs-rejected
Issue-Shape-Id: com.amazonaws.mediapackagevod#MediaPackageVod
Issue-Key: mediapackage-vod.delete-packaging-group-with-dependent-configs-rejected|_assert_rejected#8|AssertionError|raise AssertionError(f"{what}: expected {expected_code!r}, but the call succeeded and returned {result!r}")
Issue-Commit: 8da75f4f8e37c6bec2b6e28a02ca64766de7a47e

# MediaPackage VOD DeletePackagingGroup deletes a group that a PackagingConfiguration still references instead of refusing with UnprocessableEntityException

> _Note: this issue was identified against the code base at commit `8da75f4f8e37c6bec2b6e28a02ca64766de7a47e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeletePackagingGroup

## Description
One root cause on the MediaPackage VOD (restJson1, sdkId "MediaPackage Vod") control plane: DeletePackagingGroup performs no dependency check, so it destroys a packaging group while a PackagingConfiguration still references it.

=== Cause 1 of 1: DeletePackagingGroup succeeds on a group with attached PackagingConfigurations ===

AWS: Smithy (services/service-com-amazonaws-mediapackagevod-mediapackagevod/smithy-model/com-amazonaws-mediapackagevod-mediapackagevod.json): DeletePackagingGroup is bound to DELETE /packaging_groups/{Id} with required @httpLabel Id, and models UnprocessableEntityException (errorKind client, httpError 422, documented "The parameters sent in the request are not valid.") among its errors. Recorded behavior: deleting a packaging group while a PackagingConfiguration created against that PackagingGroupId still exists is REFUSED with HTTP 422 / error code UnprocessableEntityException, the group is left intact (an independent DescribePackagingGroup still returns it with its Id), and the identical delete succeeds once the dependent configuration has been deleted first. The recorded message example is "id cannot be deleted because it has attached packaging configurations"; the requirements here are the 422 status, the UnprocessableEntityException code, and the absence of any effect on the group — the message text itself is only an example.

Emulator: the delete is accepted unconditionally. The handler looks the id up in state.packaging_groups, removes it, forgets its tags and returns success — observed ResponseMetadata.HTTPStatusCode 204 with an empty body — even though a PackagingConfiguration created with PackagingGroupId = that group is still stored. There is no referential check at all, so no request can ever produce the modeled UnprocessableEntityException for this condition, the group is gone rather than intact, and the configuration is left orphaned pointing at a group that no longer exists.

Why: a client relying on AWS's referential integrity gets silent destructive behavior. Teardown and infrastructure code that deletes a group and expects 422 until it has cleaned up children never sees the refusal, so it skips deleting the configurations and leaves dangling child resources; code with error handling keyed on UnprocessableEntityException never takes that branch; and a least-destructive-operation guard ("the group is protected while it is in use") is a false guarantee against the emulator. Any test asserting the rejected delete had no effect instead finds the group deleted.

Fix: the DeletePackagingGroup handler and the packaging-group/packaging-configuration referential state it reads in the mediapackagevod service crate — before removing the group, reject with UnprocessableEntityException (422) when any stored PackagingConfiguration still carries that PackagingGroupId, and leave the group and its tags untouched on that path. NOTE for the fixer: that handler's module doc currently argues the opposite and cites a wire-corpus teardown step (mediapackage-vod-packaging-configuration-multi-format#teardown#11) that answered 204 for a group it believed still held configurations; reconcile the two rather than flipping the behavior blindly — the refusal must key off configurations that are ACTUALLY still stored at delete time, so a teardown that deletes its configurations before the group (as the behavioral check's positive control does) must still get a success, and re-replay both that fixture step and the behavioral check after the change.

Traces: mediapackage-vod.delete-packaging-group-with-dependent-configs-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the booted emulator: mediapackage-vod 15/20 passed, 5 failed. Of those 5, two are OWNED by 20261022T132356Z-mediapackage-vod-egress-manifest-url-serves-nothing-createpackaginggroup.md and one is ALREADY FILED against the unimplemented TagResource stub; this is the remaining net-new cause.

Cause 1 — mediapackage-vod.delete-packaging-group-with-dependent-configs-rejected [mismatch]
  setup:    CreatePackagingGroup(Id=<group>), then CreatePackagingConfiguration(Id=<cfg>, PackagingGroupId=<group>, HlsPackage={HlsManifests:[{ManifestName:"index"}]})
  expected: DeletePackagingGroup(Id=<group>) raises ClientError with Error.Code == "UnprocessableEntityException" (HTTP 422), and a following DescribePackagingGroup(Id=<group>) still returns Id == <group>
  actual:   AssertionError: DeletePackagingGroup while a dependent PackagingConfiguration still references the group: expected 'UnprocessableEntityException', but the call succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 204, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '2', ...}, 'RetryAttempts': 0}}
  (ids and timestamps are environment-specific examples; the requirements are the 422 status, the UnprocessableEntityException code, and the group surviving the rejected delete)

Source of the emulator behavior: services/service-com-amazonaws-mediapackagevod-mediapackagevod/src/ops/delete_packaging_group.rs — handle() at lines 160-189 removes state.packaging_groups[input.id] with no check of stored packaging configurations; SUCCESS_STATUS = 204 at line 158; the module doc at lines 19-30 states the refusal was deliberately omitted and cites wire fixture step mediapackage-vod-packaging-configuration-multi-format#teardown#11.

Model citations from services/service-com-amazonaws-mediapackagevod-mediapackagevod/smithy-model/com-amazonaws-mediapackagevod-mediapackagevod.json (restJson1, sigv4): DeletePackagingGroup DELETE /packaging_groups/{Id}, errors ForbiddenException 403, InternalServerErrorException 500, NotFoundException 404, ServiceUnavailableException 503, TooManyRequestsException 429, UnprocessableEntityException 422.

Not a duplicate of the two open mediapackage-vod issues: 20261022T132356Z-...-egress-manifest-url-serves-nothing-createpackaginggroup.md covers the egress data plane, CreatePackagingGroup authorization and the DeleteAsset success status; 20261023T044459Z-mediapackage-vod-listassets-returns-no-assets-when-filtered.md covers ListAssets filtering by packagingGroupId. Neither covers DeletePackagingGroup dependency validation.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from the checkout.
2. Replay the behavioral suite simparity-behavioral-aws-v1 (run_behavioral_suite) and watch check mediapackage-vod.delete-packaging-group-with-dependent-configs-rejected.
3. Directly: with a mediapackage-vod client, call CreatePackagingGroup(Id=<group>), then CreatePackagingConfiguration(Id=<cfg>, PackagingGroupId=<group>, HlsPackage={"HlsManifests": [{"ManifestName": "index"}]}).
4. Call DeletePackagingGroup(Id=<group>) while <cfg> still exists: observe a success response (HTTP 204) instead of a ClientError with Error.Code "UnprocessableEntityException" and HTTP 422.
5. Call DescribePackagingGroup(Id=<group>): observe NotFoundException — the group AWS would have protected has been destroyed — and DescribePackagingConfiguration(Id=<cfg>) still returns a configuration referencing the now-missing group.
6. Positive control that must keep working after the fix: DeletePackagingConfiguration(Id=<cfg>), poll until DescribePackagingConfiguration raises NotFoundException, then DeletePackagingGroup(Id=<group>) must succeed and the group must then poll to NotFoundException.
