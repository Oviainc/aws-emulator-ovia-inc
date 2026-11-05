Issue-Source: trace-sweep-divergence
Issue-Trace: s3c-access-point-policy-rejection-collapses-eight-causes-into-one-code
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3c-access-point-policy-rejection-collapses-eight-causes-into-one-code s3control-access-point-lifecycle-and-policy s3control-access-point-policy-status-envelopes s3control-account-public-access-block-lifecycle s3control-generic-tagging-absent-resource-envelopes s3control-storage-lens-group-lifecycle
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control|GetAccessPointPolicy|4xx->4xx|$.ErrorResponse.Error.AccessPointName
Issue-Key: s3control|GetAccessPointPolicy|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|GetAccessPointPolicy|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: s3control|GetAccessPointPolicyStatus|4xx->4xx|$.ErrorResponse.Error.AccessPointName
Issue-Key: s3control|GetAccessPointPolicyStatus|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|GetAccessPointPolicyStatus|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: s3control|GetPublicAccessBlock|4xx->4xx|$.ErrorResponse.Error.AccountId
Issue-Key: s3control|GetPublicAccessBlock|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|GetPublicAccessBlock|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: s3control|ListTagsForResource|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|ListTagsForResource|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: s3control|TagResource|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|TagResource|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: s3control|UntagResource|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|UntagResource|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: s3control|GetStorageLensGroup|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|GetStorageLensGroup|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: s3control|UpdateStorageLensGroup|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|UpdateStorageLensGroup|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: s3control|DeleteStorageLensGroup|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|DeleteStorageLensGroup|4xx->4xx|$.ErrorResponse.RequestId
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# s3control XML error envelopes drop RequestId, HostId and the resource-naming member

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccessPointPolicy

## Description
9 net-new root causes, one per refusing operation, all the same fault: the s3control `<ErrorResponse>` envelope is serialized without the `RequestId` and `HostId` siblings of `<Error>`, and — where the recorded refusal names the resource it could not find — without that identifying member inside `<Error>`. Status and error code match in every one of these clusters; only the envelope is short. (Open issue 20261022T213144Z-s3control-access-points-directory-bucket-listing-job-object.md covers other s3control gaps; it does not carry these replay keys.)

Shared AWS note: Smithy declares no errors for these operations (errors: []), so the envelope shape is recorded behavior only: every s3control refusal carries `<ErrorResponse><Error><Code>..</Code><Message>..</Message>[<ResourceMember>..</ResourceMember>]</Error><RequestId>..</RequestId><HostId>..</HostId></ErrorResponse>`. RequestId/HostId are per-run values — the requirement is that they are present and shaped like the service's ids, not any recorded literal; the resource member's value is taken from the request.

--- Cause 1 of 9: GetAccessPointPolicy on an absent access point ---
AWS: recorded behavior only — 404 NoSuchAccessPointPolicy/NoSuchAccessPoint carrying `<AccessPointName>` (the name from the request) plus RequestId and HostId.
Emulator: 404 with the right code but no AccessPointName, no RequestId and no HostId.
Why: a client that logs or correlates by request id has nothing to log, and tooling that reads the offending access point name out of the error must re-parse the request.
Fix: the s3control error serializer (and this operation's error construction) — populate RequestId/HostId on every refusal and include the resource member.
Traces: s3c-access-point-policy-rejection-collapses-eight-causes-into-one-code#task#3 (3 steps in cluster).

--- Cause 2 of 9: GetAccessPointPolicyStatus on an absent access point ---
AWS: recorded behavior only — 404 carrying `<AccessPointName>` from the request, plus RequestId/HostId.
Emulator: 404 without AccessPointName, RequestId or HostId.
Why: as cause 1.
Fix: the GetAccessPointPolicyStatus error path plus the shared envelope serializer.
Traces: s3control-access-point-lifecycle-and-policy#task#6 (3 steps in cluster).

--- Cause 3 of 9: GetPublicAccessBlock when no account block is configured ---
AWS: recorded behavior only — 404 NoSuchPublicAccessBlockConfiguration carrying `<AccountId>` (the account from the request's x-amz-account-id/host prefix), plus RequestId/HostId.
Emulator: 404 without AccountId, RequestId or HostId.
Why: as cause 1; the account the refusal is about is unavailable to the client.
Fix: the GetPublicAccessBlock error path plus the shared envelope serializer.
Traces: s3control-account-public-access-block-lifecycle#task#1 (2 steps in cluster).

--- Cause 4 of 9: ListTagsForResource on an absent resource ARN ---
AWS: recorded behavior only — 404 with RequestId and HostId present.
Emulator: 404 with neither.
Why: as cause 1.
Fix: the ListTagsForResource error path plus the shared envelope serializer.
Traces: s3control-generic-tagging-absent-resource-envelopes#task#3.

--- Cause 5 of 9: TagResource on an absent resource ARN ---
AWS: recorded behavior only — 404 with RequestId and HostId present.
Emulator: 404 with neither.
Why: as cause 1.
Fix: the TagResource error path plus the shared envelope serializer.
Traces: s3control-generic-tagging-absent-resource-envelopes#task#4.

--- Cause 6 of 9: UntagResource on an absent resource ARN ---
AWS: recorded behavior only — 404 with RequestId and HostId present.
Emulator: 404 with neither.
Why: as cause 1.
Fix: the UntagResource error path plus the shared envelope serializer.
Traces: s3control-generic-tagging-absent-resource-envelopes#task#5.

--- Cause 7 of 9: GetStorageLensGroup on an absent group ---
AWS: recorded behavior only — 404 with RequestId and HostId present.
Emulator: 404 with neither.
Why: as cause 1.
Fix: the GetStorageLensGroup error path plus the shared envelope serializer.
Traces: s3control-storage-lens-group-lifecycle#task#1 (2 steps in cluster).

--- Cause 8 of 9: UpdateStorageLensGroup on an absent group ---
AWS: recorded behavior only — 404 with RequestId and HostId present.
Emulator: 404 with neither.
Why: as cause 1.
Fix: the UpdateStorageLensGroup error path plus the shared envelope serializer.
Traces: s3control-storage-lens-group-lifecycle#task#9.

--- Cause 9 of 9: DeleteStorageLensGroup on an absent group ---
AWS: recorded behavior only — 404 with RequestId and HostId present.
Emulator: 404 with neither.
Why: as cause 1.
Fix: the DeleteStorageLensGroup error path plus the shared envelope serializer.
Traces: s3control-storage-lens-group-lifecycle#task#10.

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator. Statuses and codes match; the diverging keys are envelope members:
- GetAccessPointPolicy 404: `$.ErrorResponse.Error.AccessPointName: missing`, `$.ErrorResponse.RequestId: missing`, `$.ErrorResponse.HostId: missing` — s3c-access-point-policy-rejection-collapses-eight-causes-into-one-code#task#3 (3 affected steps).
- GetAccessPointPolicyStatus 404: same three keys missing — s3control-access-point-lifecycle-and-policy#task#6 (3 affected steps).
- GetPublicAccessBlock 404: `$.ErrorResponse.Error.AccountId: missing (expected '<12-digit account from the request>')` plus RequestId/HostId missing — s3control-account-public-access-block-lifecycle#task#1 (2 affected steps).
- ListTagsForResource / TagResource / UntagResource 404: `$.ErrorResponse.RequestId: missing`, `$.ErrorResponse.HostId: missing` — s3control-generic-tagging-absent-resource-envelopes#task#3/#task#4/#task#5.
- GetStorageLensGroup / UpdateStorageLensGroup / DeleteStorageLensGroup 404: RequestId and HostId missing — s3control-storage-lens-group-lifecycle#task#1 (2 affected steps), #task#9, #task#10.
The recorded RequestId/HostId literals are per-run values and are not themselves requirements; their presence and shape are.

## Steps to Reproduce
1. Replay s3c-access-point-policy-rejection-collapses-eight-causes-into-one-code#task#3: GET /v20180820/accesspoint/{Name}/policy for an access point with no policy. The emulator answers 404 whose ErrorResponse has no AccessPointName, RequestId or HostId.
2. Replay s3control-access-point-lifecycle-and-policy#task#6 (GetAccessPointPolicyStatus) and s3control-account-public-access-block-lifecycle#task#1 (GetPublicAccessBlock) — same omissions, plus the missing AccountId member.
3. Replay s3control-generic-tagging-absent-resource-envelopes#task#3..#task#5 (ListTagsForResource/TagResource/UntagResource against an ARN that does not exist) and s3control-storage-lens-group-lifecycle#task#1/#task#9/#task#10 — every 404 lacks RequestId and HostId.
