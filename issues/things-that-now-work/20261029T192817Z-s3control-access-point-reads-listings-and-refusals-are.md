Issue-Source: trace-sweep-divergence
Issue-Trace: s3c-access-point-name-validation-splits-into-two-identities
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3c-access-point-name-validation-splits-into-two-identities s3c-access-point-public-access-block-is-replaced-not-merged s3c-account-id-host-prefix-is-authorized-before-the-request s3control-access-point-create-rejections s3control-access-point-lifecycle-and-policy s3control-access-point-listing-pagination-and-bucket-scoping
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control|GetAccessPoint|2xx->2xx|$.GetAccessPointResult.AccessPointType
Issue-Key: s3control|GetAccessPoint|2xx->2xx|$.GetAccessPointResult.BucketAccountId
Issue-Key: s3control|GetAccessPoint|2xx->2xx|$.GetAccessPointResult.Endpoints
Issue-Key: s3control|CreateAccessPoint|4xx->2xx|$.ErrorResponse
Issue-Key: s3control|CreateAccessPoint|4xx->2xx|status
Issue-Key: s3control|CreateAccessPoint|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: s3control|CreateAccessPoint|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: s3control|CreateAccessPoint|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|CreateAccessPoint|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: s3control|GetAccessPoint|4xx->4xx|$.ErrorResponse.Error.AccessPointName
Issue-Key: s3control|ListAccessPoints|2xx->2xx|$.ListAccessPointsResult.AccessPointList.AccessPoint.AccessPointType
Issue-Key: s3control|ListAccessPoints|2xx->2xx|$.ListAccessPointsResult.AccessPointList.AccessPoint.BucketAccountId
Issue-Key: s3control|DeleteAccessPoint|4xx->4xx|$.ErrorResponse.Error.AccessPointName
Issue-Key: s3control|DeleteAccessPoint|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: s3control|DeleteAccessPoint|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|DeleteAccessPoint|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: s3control|ListAccessPoints|2xx->2xx|$.ListAccessPointsResult.AccessPointList.AccessPoint[*].AccessPointType
Issue-Key: s3control|ListAccessPoints|2xx->2xx|$.ListAccessPointsResult.AccessPointList.AccessPoint[*].BucketAccountId
Issue-Related: 20261029T192817Z-s3control-ignores-the-accountid-host-prefix-so-cross.md
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# s3control access point reads, listings and refusals are incomplete and wrongly coded

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccessPoint

## Description
9 net-new root causes on the s3control access-point surface: three response projections drop members AWS always sends, three refusal messages diverge from AWS's fixed text, one absent access point is reported as present, and two CreateAccessPoint rejections are missed or mis-coded. Smithy declares no errors for these operations (errors: []), so the codes and messages below are recorded behavior only; the response members are cited from the model where it has them.

--- Cause 1 of 9: GetAccessPoint omits Endpoints, BucketAccountId and AccessPointType ---
AWS: Smithy: GetAccessPoint output members `Endpoints` (target Endpoints, a map) and `BucketAccountId` (target AccountId, @pattern ^\d{12}$); `AccessPointType` is recorded behavior only (the wire sends it, the on-disk model does not list it). The recording shows Endpoints as the four regional entries keyed `ipv4`, `fips`, `fips_dualstack`, `dualstack` (hostnames derived from the service and region, not literals), BucketAccountId = the owning account, AccessPointType = `Regular` for a non-outpost access point.
Emulator: returns none of the three.
Why: a client that resolves the access point's data-plane endpoint from Endpoints has nothing to resolve, and one that checks the bucket's owning account or the access point's type cannot.
Fix: the GetAccessPoint response projection in the s3control crate — emit Endpoints, BucketAccountId and AccessPointType from the access point's state.
Traces: s3c-access-point-name-validation-splits-into-two-identities#task#3 (8 steps in cluster).

--- Cause 2 of 9: CreateAccessPoint accepts a name the wire cannot address ---
AWS: recorded behavior only — a name longer than the addressable limit is refused at URI parse time with HTTP 400 and `<Error><Code>InvalidURI</Code><Message>Couldn't parse the specified URI.</Message><URI>..</URI></Error>` plus RequestId/HostId. Fixed text: code `InvalidURI`, message `Couldn't parse the specified URI.`; the variable `URI` is the request path, `/v20180820/accesspoint/<name>`.
Emulator: answers 200 and creates the access point.
Why: the client believes an unusable name was accepted; the resulting access point has no AWS counterpart and any name-boundary probe passes when AWS would fail.
Fix: CreateAccessPoint request validation / the URI-parsing stage ahead of it.
Traces: s3c-access-point-name-validation-splits-into-two-identities#task#4 (6 steps in cluster).

--- Cause 3 of 9: an unparsable PublicAccessBlockConfiguration boolean yields the wrong code and message ---
AWS: recorded behavior only — a non-boolean value inside PublicAccessBlockConfiguration is refused with HTTP 400, Code `InvalidRequest`, Message `Request invalid` (both fixed text, with no interpolation), plus RequestId/HostId.
Emulator: answers 400 with Code `ValidationException` and Message `invalid boolean: 'maybe'`, and omits RequestId/HostId.
Why: a client branching on `InvalidRequest` does not recognize the refusal, and the message leaks a request value into text AWS keeps fixed.
Fix: CreateAccessPoint input parsing/validation plus the error construction — map body parse failures to InvalidRequest / `Request invalid`.
Traces: s3c-access-point-public-access-block-is-replaced-not-merged#task#10 (2 steps in cluster).

--- Cause 4 of 9: GetAccessPoint's not-found message interpolates the name ---
AWS: recorded behavior only — 404 NoSuchAccessPoint with Message exactly `The specified accesspoint does not exist` (fixed text, no name in it) and `<AccessPointName>` carrying the requested name, plus RequestId/HostId.
Emulator: answers `The specified access point does not exist: <name>` and omits AccessPointName/RequestId/HostId.
Why: the message differs from AWS's wording (including AWS's unspaced `accesspoint`), so message matching fails, and the name is not available where clients read it.
Fix: the GetAccessPoint refusal construction plus the shared envelope serializer.
Traces: s3c-account-id-host-prefix-is-authorized-before-the-request#task#6 (2 steps in cluster).

--- Cause 5 of 9: ListAccessPoints omits AccessPointType and BucketAccountId (single-entry form) ---
AWS: recorded behavior only for AccessPointType (`Regular`); BucketAccountId is the owning account. Both appear on every AccessPoint entry of the listing.
Emulator: omits both from the entry.
Why: a client that filters a listing by access point type or bucket owner sees unset fields and must call GetAccessPoint per entry — which (cause 1) also omits them.
Fix: the ListAccessPoints entry projection, shared with GetAccessPoint's.
Traces: s3c-account-id-host-prefix-is-authorized-before-the-request#task#9 (3 steps in cluster).

--- Cause 6 of 9: CreateAccessPoint's duplicate-name refusal uses the wrong message ---
AWS: recorded behavior only — recreating an access point you own is refused with HTTP 409 and Message exactly `Your previous request to create the named accesspoint succeeded and you already own it.` (fixed text, no name interpolated), plus RequestId/HostId.
Emulator: answers 409 with `An access point with the same name already exists: <name>` and omits RequestId/HostId.
Why: the message is not AWS's; clients and tests that match it (the conventional S3 idempotent-create wording) do not recognize the condition.
Fix: the CreateAccessPoint conflict refusal construction plus the envelope serializer.
Traces: s3control-access-point-create-rejections#task#4.

--- Cause 7 of 9: GetAccessPoint answers 200 for an access point that was never created ---
AWS: recorded behavior only — 404 with Code `NoSuchAccessPoint`, Message `The specified accesspoint does not exist` and `<AccessPointName>` from the request.
Emulator: answers HTTP 200 with no error body.
Why: a client cannot tell a missing access point from an existing one, so existence checks and create-if-absent flows take the wrong branch.
Fix: the GetAccessPoint lookup path — refuse an unknown name instead of answering success.
Traces: s3control-access-point-create-rejections#task#6.

--- Cause 8 of 9: DeleteAccessPoint's not-found message interpolates the name ---
AWS: recorded behavior only — 404 with Message `The specified accesspoint does not exist` and `<AccessPointName>` from the request, plus RequestId/HostId.
Emulator: answers `The specified access point does not exist: <name>` and omits AccessPointName/RequestId/HostId.
Why: as cause 4.
Fix: the DeleteAccessPoint refusal construction plus the envelope serializer.
Traces: s3control-access-point-lifecycle-and-policy#teardown#15.

--- Cause 9 of 9: ListAccessPoints omits AccessPointType and BucketAccountId on every entry of a multi-entry listing ---
AWS: recorded behavior only for AccessPointType (`Regular`) and the owning account in BucketAccountId, present on each of the listed entries.
Emulator: omits both from every entry (observed on a two-entry listing), so the gap is in the repeated projection, not only in the single-entry shape.
Why: as cause 5, for the paginated/bucket-scoped listing path.
Fix: the ListAccessPoints list-member projection.
Traces: s3control-access-point-listing-pagination-and-bucket-scoping#task#6.

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator:
- GetAccessPoint 200: `$.GetAccessPointResult.Endpoints: missing (expected the ipv4/fips/fips_dualstack/dualstack entries, got None)`, `BucketAccountId: missing`, `AccessPointType: missing (expected 'Regular')` — s3c-access-point-name-validation-splits-into-two-identities#task#3 (8 affected steps).
- CreateAccessPoint over-long name: `status: expected 400, got 200`; expected body `{'Error': {'Code': 'InvalidURI', 'Message': "Couldn't parse the specified URI.", 'URI': '/v20180820/accesspoint/<name>'}, ...}` — same fixture task#4 (6 affected steps).
- CreateAccessPoint bad boolean: `Code: expected 'InvalidRequest', got 'ValidationException'`; `Message: expected 'Request invalid', got "invalid boolean: 'maybe'"`; RequestId/HostId missing — s3c-access-point-public-access-block-is-replaced-not-merged#task#10 (2 affected steps).
- GetAccessPoint 404: `Message: expected 'The specified accesspoint does not exist', got 'The specified access point does not exist: <name>'`; `Error.AccessPointName: missing`; RequestId/HostId missing — s3c-account-id-host-prefix-is-authorized-before-the-request#task#6 (2 affected steps).
- ListAccessPoints 200: `AccessPointList.AccessPoint.AccessPointType: missing (expected 'Regular')`, `...BucketAccountId: missing` — s3c-account-id-host-prefix-is-authorized-before-the-request#task#9 (3 affected steps); and the same two keys missing on `AccessPoint[0]` and `AccessPoint[1]` — s3control-access-point-listing-pagination-and-bucket-scoping#task#6.
- CreateAccessPoint duplicate: `Message: expected 'Your previous request to create the named accesspoint succeeded and you already own it.', got 'An access point with the same name already exists: <name>'` at 409 — s3control-access-point-create-rejections#task#4.
- GetAccessPoint absent: `status: expected 404, got 200`; expected `{'Error': {'Code': 'NoSuchAccessPoint', 'Message': 'The specified accesspoint does not exist', 'AccessPointName': '<name>'}, ...}` — same fixture task#6.
- DeleteAccessPoint absent: `Message: expected 'The specified accesspoint does not exist', got 'The specified access point does not exist: <name>'`; AccessPointName/RequestId/HostId missing — s3control-access-point-lifecycle-and-policy#teardown#15.

## Steps to Reproduce
1. Replay s3c-access-point-name-validation-splits-into-two-identities#task#3: create an access point and GET /v20180820/accesspoint/{Name} — the response has no Endpoints, BucketAccountId or AccessPointType.
2. Same fixture task#4: PUT /v20180820/accesspoint/<over-long name> — the emulator answers 200 instead of 400 InvalidURI.
3. s3c-access-point-public-access-block-is-replaced-not-merged#task#10: create an access point with a PublicAccessBlockConfiguration boolean of `maybe` — the emulator answers ValidationException / `invalid boolean: 'maybe'` instead of InvalidRequest / `Request invalid`.
4. s3c-account-id-host-prefix-is-authorized-before-the-request#task#6 and s3control-access-point-lifecycle-and-policy#teardown#15: GET/DELETE an access point that does not exist — the message interpolates the name and AccessPointName is absent.
5. s3c-account-id-host-prefix-is-authorized-before-the-request#task#9 and s3control-access-point-listing-pagination-and-bucket-scoping#task#6: GET /v20180820/accesspoint — no entry carries AccessPointType or BucketAccountId.
6. s3control-access-point-create-rejections#task#4 and #task#6: re-create an existing access point (wrong 409 message), then GET a name never created (200 instead of 404 NoSuchAccessPoint).
