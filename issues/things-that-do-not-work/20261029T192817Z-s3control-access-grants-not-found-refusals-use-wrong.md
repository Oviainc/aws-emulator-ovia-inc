Issue-Source: trace-sweep-divergence
Issue-Trace: s3control-access-grants-grant-lifecycle-and-location-referential-integrity
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3control-access-grants-grant-lifecycle-and-location-referential-integrity s3control-access-grants-location-lifecycle s3control-access-grants-instance-lifecycle s3control-access-grants-operations-require-an-instance s3control-access-grants-instance-resource-policy-lifecycle
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control|DeleteAccessGrantsLocation|4xx->4xx|$.ErrorResponse.Error.AccessGrantsLocationID
Issue-Key: s3control|DeleteAccessGrantsLocation|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|DeleteAccessGrantsLocation|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: s3control|DeleteAccessGrantsLocation|4xx->4xx|status
Issue-Key: s3control|GetAccessGrant|4xx->4xx|$.ErrorResponse.Error.AccessGrantId
Issue-Key: s3control|GetAccessGrant|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|GetAccessGrant|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: s3control|GetAccessGrantsLocation|4xx->4xx|$.ErrorResponse.Error.AccessGrantsLocationId
Issue-Key: s3control|GetAccessGrantsLocation|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|GetAccessGrantsLocation|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: s3control|GetAccessGrantsInstance|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: s3control|GetAccessGrantsInstance|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|GetAccessGrantsInstance|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: s3control|GetAccessGrantsInstance|4xx->2xx|$.ErrorResponse
Issue-Key: s3control|GetAccessGrantsInstance|4xx->2xx|status
Issue-Key: s3control|GetAccessGrantsInstanceResourcePolicy|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|GetAccessGrantsInstanceResourcePolicy|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: s3control|DeleteAccessGrantsLocation|4xx->4xx|$.ErrorResponse.Error.AccessGrantsLocationId
Issue-Key: s3control|GetAccessGrantsInstanceResourcePolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# s3control Access Grants not-found refusals use wrong status, code and interpolated messages

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccessGrantsInstance

## Description
8 net-new root causes on the s3control Access Grants surface, all in its refusal path: one wrong HTTP status, one deleted resource that still resolves, two refusal messages that interpolate the identifier instead of using AWS's fixed text, and four envelopes missing their identifying member and/or RequestId/HostId. Smithy declares no errors for these operations (errors: []), so every expectation below is recorded behavior only.

--- Cause 1 of 8: DeleteAccessGrantsLocation with grants still attached answers 409 instead of 400 ---
AWS: recorded behavior only — deleting a location that still has grants is refused with HTTP 400, and the `<Error>` carries `<AccessGrantsLocationID>` (the location id from the request) plus RequestId/HostId.
Emulator: answers HTTP 409 and omits AccessGrantsLocationID, RequestId and HostId.
Why: a client branching on the status treats 409 as a retryable conflict and 400 as a request it must change; the referential-integrity refusal is therefore mis-classified, and the id it is about is not in the body.
Fix: the DeleteAccessGrantsLocation refusal path (status selection) plus the shared s3control error envelope serializer.
Traces: s3control-access-grants-grant-lifecycle-and-location-referential-integrity#task#10.

--- Cause 2 of 8: GetAccessGrant on a deleted grant omits AccessGrantId/RequestId/HostId ---
AWS: recorded behavior only — 404 whose `<Error>` carries `<AccessGrantId>` (the id from the request) plus RequestId/HostId.
Emulator: 404 with the right code but none of those three members.
Why: the client cannot tell which grant the refusal names and has no request id to correlate.
Fix: the GetAccessGrant error path plus the shared envelope serializer.
Traces: s3control-access-grants-grant-lifecycle-and-location-referential-integrity#task#12.

--- Cause 3 of 8: GetAccessGrantsLocation on a deleted location omits AccessGrantsLocationId/RequestId/HostId ---
AWS: recorded behavior only — 404 carrying `<AccessGrantsLocationId>` from the request plus RequestId/HostId.
Emulator: 404 without them.
Why: as cause 2.
Fix: the GetAccessGrantsLocation error path plus the shared envelope serializer.
Traces: s3control-access-grants-grant-lifecycle-and-location-referential-integrity#task#14 (2 steps in cluster).

--- Cause 4 of 8: GetAccessGrantsInstance interpolates its not-found message ---
AWS: recorded behavior only — when the account has no Access Grants instance in the region, the refusal is 404 AccessGrantsInstanceNotExistsError with the message exactly `Access Grants Instance does not exist` — fixed text with no variable part; the account and region are not interpolated.
Emulator: answers `Access Grants Instance does not exist: account <accountId> has no S3 Access Grants instance in <region>.`, and omits RequestId/HostId.
Why: a client that matches the documented message (or a test asserting on it) fails, and the emulator leaks request-derived values into text AWS keeps fixed.
Fix: the GetAccessGrantsInstance refusal construction — use the fixed message, and carry the identifiers in the envelope's members instead.
Traces: s3control-access-grants-instance-lifecycle#task#1 (2 steps in cluster).

--- Cause 5 of 8: GetAccessGrantsInstance still answers 200 after DeleteAccessGrantsInstance ---
AWS: recorded behavior only — after the instance is deleted, GetAccessGrantsInstance answers 404 with Code `AccessGrantsInstanceNotExistsError` and Message `Access Grants Instance does not exist`.
Emulator: answers 200 with the instance still described.
Why: the delete appears to have no effect; a client cannot verify teardown, and idempotent create/delete flows loop or double-create.
Fix: the Access Grants instance state — DeleteAccessGrantsInstance must remove the instance so reads refuse it.
Traces: s3control-access-grants-instance-lifecycle#task#7.

--- Cause 6 of 8: GetAccessGrantsInstanceResourcePolicy absent-policy refusal omits RequestId/HostId ---
AWS: recorded behavior only — 404 whose envelope carries RequestId and HostId.
Emulator: 404 without them.
Why: no correlation id for the client to log or report.
Fix: the GetAccessGrantsInstanceResourcePolicy error path plus the shared envelope serializer.
Traces: s3control-access-grants-instance-resource-policy-lifecycle#task#2 (2 steps in cluster).

--- Cause 7 of 8: DeleteAccessGrantsLocation on a deleted location omits AccessGrantsLocationId/RequestId/HostId ---
AWS: recorded behavior only — 404 carrying `<AccessGrantsLocationId>` from the request plus RequestId/HostId.
Emulator: 404 without them.
Why: as cause 2.
Fix: the DeleteAccessGrantsLocation error path plus the shared envelope serializer.
Traces: s3control-access-grants-location-lifecycle#teardown#13.

--- Cause 8 of 8: GetAccessGrantsInstanceResourcePolicy interpolates the no-instance message ---
AWS: recorded behavior only — with no Access Grants instance the refusal is 404 with the fixed message `Access Grants Instance does not exist`.
Emulator: answers `Access Grants Instance does not exist: account <accountId> has no S3 Access Grants instance in <region>.` and omits RequestId/HostId.
Why: as cause 4.
Fix: the shared no-instance refusal used by the resource-policy operations.
Traces: s3control-access-grants-operations-require-an-instance#task#3.

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator:
- DeleteAccessGrantsLocation (location still referenced): `status: expected 400, got 409`; `$.ErrorResponse.Error.AccessGrantsLocationID: missing`; RequestId/HostId missing — s3control-access-grants-grant-lifecycle-and-location-referential-integrity#task#10.
- GetAccessGrant on a deleted grant: `$.ErrorResponse.Error.AccessGrantId: missing` + RequestId/HostId missing (404) — same fixture task#12.
- GetAccessGrantsLocation on a deleted location: `$.ErrorResponse.Error.AccessGrantsLocationId: missing` + RequestId/HostId missing (404) — same fixture task#14 (2 affected steps).
- GetAccessGrantsInstance with no instance: `$.ErrorResponse.Error.Message: value_mismatch (expected 'Access Grants Instance does not exist', got 'Access Grants Instance does not exist: account 000000000000 has no S3 Access Grants instance in us-west-2.')` — s3control-access-grants-instance-lifecycle#task#1 (2 affected steps).
- GetAccessGrantsInstance after delete: `status: expected 404, got 200`; `$.ErrorResponse: missing (expected {'Error': {'Code': 'AccessGrantsInstanceNotExistsError', 'Message': 'Access Grants Instance does not exist'}, ...})` — same fixture task#7.
- GetAccessGrantsInstanceResourcePolicy (absent policy): RequestId/HostId missing at 404 — s3control-access-grants-instance-resource-policy-lifecycle#task#2 (2 affected steps).
- DeleteAccessGrantsLocation (already deleted): AccessGrantsLocationId + RequestId/HostId missing at 404 — s3control-access-grants-location-lifecycle#teardown#13.
- GetAccessGrantsInstanceResourcePolicy with no instance: same interpolated message divergence — s3control-access-grants-operations-require-an-instance#task#3.
The recorded RequestId/HostId literals are per-run values; only their presence is a requirement.

## Steps to Reproduce
1. Replay s3control-access-grants-instance-lifecycle#task#1: with no Access Grants instance, GET /v20180820/accessgrantsinstance — the emulator's message interpolates account and region.
2. Same fixture task#7: CreateAccessGrantsInstance, DeleteAccessGrantsInstance, then GetAccessGrantsInstance — the emulator answers 200 instead of 404 AccessGrantsInstanceNotExistsError.
3. Replay s3control-access-grants-grant-lifecycle-and-location-referential-integrity#task#10: create a location plus a grant on it, then DELETE the location — the emulator answers 409 instead of 400 and omits AccessGrantsLocationID.
4. Replay the same fixture's task#12/#task#14, s3control-access-grants-location-lifecycle#teardown#13 and s3control-access-grants-instance-resource-policy-lifecycle#task#2: every 404 omits its identifying member and RequestId/HostId.
5. Replay s3control-access-grants-operations-require-an-instance#task#3 for the interpolated no-instance message on GetAccessGrantsInstanceResourcePolicy.
