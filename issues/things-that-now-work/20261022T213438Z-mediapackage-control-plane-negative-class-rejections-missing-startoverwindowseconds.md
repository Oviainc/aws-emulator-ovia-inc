Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mediapackage.origin-endpoint-startover-window-negative-rejected mediapackage.create-harvest-job-invalid-time-range-rejected mediapackage.create-harvest-job-unassumable-role-forbidden mediapackage.rotate-channel-credentials-nonexistent-channel-not-found mediapackage.rotate-ingest-endpoint-credentials-invalid-endpoint-id-rejected
Issue-Shape-Id: com.amazonaws.mediapackage#MediaPackage
Issue-Key: mediapackage.origin-endpoint-startover-window-negative-rejected|_error_code#6|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: mediapackage.create-harvest-job-invalid-time-range-rejected|run#10|AssertionError|assert code == "UnprocessableEntityException", f"expected UnprocessableEntityException, got {code!r}"
Issue-Key: mediapackage.create-harvest-job-unassumable-role-forbidden|run#24|AssertionError|assert code == "UnprocessableEntityException", f"unexpected error code: {code!r}"
Issue-Key: mediapackage.rotate-channel-credentials-nonexistent-channel-not-found|run#4|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code!r}"
Issue-Key: mediapackage.rotate-ingest-endpoint-credentials-invalid-endpoint-id-rejected|run#10|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code!r}"
Issue-Commit: bd3308b0c8d992d57414e8fe0564c68280e702c4

# MediaPackage control plane: negative-class rejections missing — StartoverWindowSeconds<0 accepted, CreateHarvestJob validations and Rotate*Credentials NotFound semantics absent (5 causes)

> _Note: this issue was identified against the code base at commit `bd3308b0c8d992d57414e8fe0564c68280e702c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateOriginEndpoint, CreateHarvestJob, RotateChannelCredentials, RotateIngestEndpointCredentials

## Description
Five distinct control-plane root causes in the mediapackage emulator (crate services/service-com-amazonaws-mediapackage-mediapackage). All five are negative-class rejections real MediaPackage performs and the emulator does not. Causes 2-5 sit on operations whose scaffold stubs are also filed separately as "unimplemented scaffold stub" issues (CreateHarvestJob, RotateChannelCredentials, RotateIngestEndpointCredentials); implementing the stub alone does NOT close these causes — each names the specific rejection the implementation must carry, and must be re-replayed with the check ids listed per cause.

Cause 1 of 5: CreateOriginEndpoint accepts a negative StartoverWindowSeconds.
AWS: Smithy model (com-amazonaws-mediapackage-mediapackage.json) — CreateOriginEndpoint.StartoverWindowSeconds targets __integer (a startover window is a duration in seconds, so a negative value is not a representable window); the operation models UnprocessableEntityException (httpError 422) for semantically invalid input. Recorded behavior: the create is refused with a client error and no endpoint is created.
Emulator: CreateOriginEndpoint(Id=..., ChannelId=..., HlsPackage={}, StartoverWindowSeconds=-100) returns HTTP 200 and persists the endpoint with 'StartoverWindowSeconds': -100 echoed back in the response body (full 200 body captured in the replay output).
Why: a client that mis-signs or mis-computes a window gets a silently created endpoint carrying a nonsensical negative startover window instead of a 422 it can handle; any client that relies on validation-at-create to reject bad configuration ships a broken endpoint.
Fix: input validation of CreateOriginEndpoint (and symmetrically UpdateOriginEndpoint) in the operation's handler/validation layer: reject StartoverWindowSeconds < 0 with UnprocessableEntityException / HTTP 422 and persist nothing.
Traces: mediapackage.origin-endpoint-startover-window-negative-rejected

Cause 2 of 5: CreateHarvestJob does not reject EndTime <= StartTime.
AWS: Smithy — CreateHarvestJob requires StartTime, EndTime, Id, OriginEndpointId, S3Destination and models UnprocessableEntityException (httpError 422). Recorded behavior: a harvest job whose EndTime precedes its StartTime is refused with UnprocessableEntityException (422) and no job is persisted under that Id (a follow-up DescribeHarvestJob returns NotFoundException / 404). The recorded message "endTime must be later than the start time" is an example of a request-derived validation message, not a byte requirement.
Emulator: the call returns error code 'NotImplemented' ("operation CreateHarvestJob is scaffolded but not implemented in this emulator") instead of UnprocessableEntityException, so the time-range rule does not exist.
Why: a client probing input validity cannot distinguish "your time range is invalid" from "this API is absent"; retry/backoff logic keyed on 422 vs 5xx-ish NotImplemented behaves wrongly, and no harvest-job negative class is enforceable.
Fix: CreateHarvestJob handler (services/service-com-amazonaws-mediapackage-mediapackage/src/ops/create_harvest_job.rs) plus its validation: once implemented, validate the StartTime/EndTime ordering and answer 422 UnprocessableEntityException, persisting nothing; DescribeHarvestJob must then answer NotFoundException for that Id.
Traces: mediapackage.create-harvest-job-invalid-time-range-rejected

Cause 3 of 5: CreateHarvestJob does not reject an S3Destination RoleArn MediaPackage cannot assume.
AWS: Smithy — CreateHarvestJob models both ForbiddenException (403) and UnprocessableEntityException (422); recorded behavior is a rejection with one of those two codes (the recording shows UnprocessableEntityException whose message mentions the role not being assumable) when the supplied role cannot be assumed by the mediapackage service principal / lacks the destination-bucket permissions.
Emulator: error code 'NotImplemented' again, so a harvest job pointed at an unusable role is never refused.
Why: an "overly permissive" divergence — a client gets no signal that its harvest destination role is unusable, which on AWS is a hard pre-flight failure rather than a silently broken job.
Fix: CreateHarvestJob handler/validation in the same file: validate the S3Destination RoleArn against the emulator's IAM state (trust policy for mediapackage.amazonaws.com and bucket permissions) and answer ForbiddenException/403 or UnprocessableEntityException/422 as recorded.
Traces: mediapackage.create-harvest-job-unassumable-role-forbidden

Cause 4 of 5: RotateChannelCredentials on a nonexistent channel does not answer NotFoundException.
AWS: Smithy — RotateChannelCredentials (PUT /channels/{Id}/credentials, deprecated in favour of RotateIngestEndpointCredentials) models NotFoundException with httpError 404. Recorded behavior: rotating credentials for a channel Id that does not exist is refused with NotFoundException / 404.
Emulator: error code 'NotImplemented' instead of 'NotFoundException'.
Why: existence checks are the primary way clients discover a deleted/mistyped channel; a non-modelled error code breaks error handling and hides the 404 class entirely.
Fix: RotateChannelCredentials handler (src/ops/rotate_channel_credentials.rs) plus channel state lookup: resolve Id against channel state first and return NotFoundException/404 when absent.
Traces: mediapackage.rotate-channel-credentials-nonexistent-channel-not-found

Cause 5 of 5: RotateIngestEndpointCredentials with an IngestEndpointId that does not belong to the channel does not answer NotFoundException.
AWS: Smithy — RotateIngestEndpointCredentials (PUT /channels/{Id}/ingest_endpoints/{IngestEndpointId}/credentials) takes both Id and IngestEndpointId as required httpLabel members and models NotFoundException with httpError 404. Recorded behavior: an IngestEndpointId not present in the channel's HlsIngest.IngestEndpoints is refused with NotFoundException/404 and the channel's real ingest Username/Password are left untouched (verified by a following DescribeChannel). The recorded message ("channel with id=<id> does not have Ingest Endpoint with guid = <guid> ", trailing space included) is an example of a request-derived message, not a byte requirement.
Emulator: error code 'NotImplemented' instead of 'NotFoundException'; the rejection path and the "rejected rotation must not disturb real credentials" invariant are therefore unverifiable.
Why: a client that rotates the wrong endpoint id gets an unmodelled error instead of a 404, and has no guarantee that a failed rotation left the live ingest credentials intact.
Fix: RotateIngestEndpointCredentials handler (src/ops/rotate_ingest_endpoint_credentials.rs): resolve IngestEndpointId within the channel's ingest-endpoint state, return NotFoundException/404 when it is not a member, and mutate nothing on that path.
Traces: mediapackage.rotate-ingest-endpoint-credentials-invalid-endpoint-id-rejected

## Evidence
Behavioral suite (simparity-behavioral-aws-v1) run against the built emulator, service mediapackage: 35/58 passed, 23 failed.

Cause 1 — [mismatch] mediapackage.origin-endpoint-startover-window-negative-rejected:
  expected: ClientError (UnprocessableEntityException, HTTP 422)
  actual:   AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'Id': 'sim2real-mp-oe-...', 'StartoverWindowSeconds': -100, 'Origination': 'ALLOW', ...}

Cause 2 — [mismatch] mediapackage.create-harvest-job-invalid-time-range-rejected:
  expected: error code 'UnprocessableEntityException'
  actual:   AssertionError: expected UnprocessableEntityException, got 'NotImplemented'

Cause 3 — [mismatch] mediapackage.create-harvest-job-unassumable-role-forbidden:
  expected: error code 'UnprocessableEntityException' (ForbiddenException also modelled/retryable)
  actual:   AssertionError: unexpected error code: 'NotImplemented'

Cause 4 — [mismatch] mediapackage.rotate-channel-credentials-nonexistent-channel-not-found:
  expected: error code 'NotFoundException'
  actual:   AssertionError: expected NotFoundException, got 'NotImplemented'

Cause 5 — [mismatch] mediapackage.rotate-ingest-endpoint-credentials-invalid-endpoint-id-rejected:
  expected: error code 'NotFoundException'
  actual:   AssertionError: expected NotFoundException, got 'NotImplemented'

Model evidence: smithy_query(operation="CreateOriginEndpoint,CreateHarvestJob,RotateChannelCredentials,RotateIngestEndpointCredentials") on services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json — every one of these four operations declares errors [ForbiddenException 403, InternalServerErrorException 500, NotFoundException 404, ServiceUnavailableException 503, TooManyRequestsException 429, UnprocessableEntityException 422].

Note for the fixer: the separately filed "unimplemented scaffold stub" issues for CreateHarvestJob, RotateChannelCredentials and RotateIngestEndpointCredentials cover standing the operations up; they do not cover causes 2-5's rejection semantics, which must be re-replayed with the check ids in trace_ids.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator, then point boto3 at it (region us-west-2).
2. Cause 1: mp.create_channel(Id="c1"); mp.create_origin_endpoint(Id="oe1", ChannelId="c1", HlsPackage={}, StartoverWindowSeconds=-100) -> observe HTTP 200 with StartoverWindowSeconds=-100; AWS refuses with UnprocessableEntityException/422.
3. Cause 2: create a channel + origin endpoint (StartoverWindowSeconds=86400), an S3 bucket and an assumable role, then mp.create_harvest_job(Id="hj1", OriginEndpointId="oe1", StartTime=T, EndTime=T-60s, S3Destination={...}) -> observe error code NotImplemented; AWS returns UnprocessableEntityException/422 and DescribeHarvestJob(Id="hj1") then returns NotFoundException.
4. Cause 3: same as 3 but with a valid time range and a RoleArn whose trust policy does not allow mediapackage.amazonaws.com -> observe NotImplemented; AWS returns UnprocessableEntityException/422 (or ForbiddenException/403).
5. Cause 4: mp.rotate_channel_credentials(Id="does-not-exist") -> observe NotImplemented; AWS returns NotFoundException/404.
6. Cause 5: mp.create_channel(Id="c2"); mp.rotate_ingest_endpoint_credentials(Id="c2", IngestEndpointId="not-a-real-ingest-endpoint-id") -> observe NotImplemented; AWS returns NotFoundException/404 and a following DescribeChannel shows the real ingest Username/Password unchanged.
7. Or replay directly: run_behavioral_suite and re-run the five check ids in trace_ids.
