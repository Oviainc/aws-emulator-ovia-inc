Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mediapackage.harvest-job-list-filters-reflect-status mediapackage.origin-endpoint-startover-window-negative-rejected mediapackage.rotate-ingest-endpoint-credentials-invalid-endpoint-id-rejected
Issue-Shape-Id: com.amazonaws.mediapackage#MediaPackage
Issue-Key: mediapackage.harvest-job-list-filters-reflect-status|_error_code_and_message#21|AssertionError|raise AssertionError(f"real AWS refuses this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: mediapackage.origin-endpoint-startover-window-negative-rejected|run#9|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: mediapackage.rotate-ingest-endpoint-credentials-invalid-endpoint-id-rejected|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# MediaPackage control plane: ListHarvestJobs accepts the mutually-exclusive IncludeChannelId+IncludeStatus pair, CreateOriginEndpoint enforces startoverWindowSeconds >= 0 instead of >= 300, and RotateIngestEndpointCredentials' NotFoundException message drops the channel id (3 causes)

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListHarvestJobs

## Description
Three distinct control-plane fidelity defects in the mediapackage emulator, found by replaying the behavioral suite (48/58 passed). They are bundled because they all belong to the same service; each is a separate root cause and must be closed separately.

Cause 1 of 3: ListHarvestJobs does not enforce the mutual exclusion of IncludeChannelId and IncludeStatus.
AWS: recorded behavior only — Smithy (ListHarvestJobs, com-amazonaws-mediapackage-mediapackage.json) models IncludeChannelId (httpQuery includeChannelId) and IncludeStatus (httpQuery includeStatus) as independent optional query parameters and says nothing about combining them; the recording shows real MediaPackage REFUSES the combination outright rather than intersecting the two filters, answering the modeled UnprocessableEntityException (httpError 422), with a message naming the exclusion (recorded example: "includeStatus cannot be specified at the same time as IncludeChannelId", measured 2026-09-20 us-west-2). The requirement is the rejection with error code UnprocessableEntityException / HTTP 422, not the exact wording.
Emulator: the call SUCCEEDS with HTTP 200 and returns the intersected/unfiltered job list (replay returned {'HarvestJobs': [{... 'Status': 'IN_PROGRESS' ...}]} for ListHarvestJobs(IncludeChannelId=<channel>, IncludeStatus="IN_PROGRESS")). The IncludeStatus-only and IncludeChannelId-only paths behave correctly.
Why: a client that (correctly) treats the combination as invalid gets no signal that its filter arguments are contradictory; code written against the emulator will pass both parameters and silently receive a different result set on real AWS, which rejects the request. Over-permissive acceptance also hides client bugs until production.
Fix: the ListHarvestJobs handler's request validation — reject when both includeChannelId and includeStatus are present, before any store query, with UnprocessableEntityException/422.
Traces: mediapackage.harvest-job-list-filters-reflect-status.

Cause 2 of 3: CreateOriginEndpoint validates startoverWindowSeconds against a minimum of 0 instead of MediaPackage's minimum of 300.
AWS: recorded behavior only — Smithy models StartoverWindowSeconds as a plain __integer (jsonName startoverWindowSeconds) with no @range, so the bound is service-side. The recording shows CreateOriginEndpoint rejecting an out-of-range value with UnprocessableEntityException (httpError 422) and a message built from the request member and its constraint: "startoverWindowSeconds must be greater than or equal to 300". The requirement is the bound itself (minimum 300) plus the 422/UnprocessableEntityException classification; the message is the parameterized member+constraint form, not fixed prose.
Emulator: rejects with the right code but states, and therefore enforces, a different bound: "startoverWindowSeconds must be greater than or equal to 0". Consequently values in 1..299 — which real MediaPackage refuses — are accepted and persisted by the emulator.
Why: a client tuning a startover window to e.g. 60 seconds is accepted by the emulator and then rejected by AWS at deploy time; conversely a client relying on the emulator's error text sees the wrong permitted range. The accepted domain of the API differs from AWS's.
Fix: the CreateOriginEndpoint (and the matching UpdateOriginEndpoint) validation for startoverWindowSeconds — raise the minimum to 300 and derive the message from the member name and that bound.
Traces: mediapackage.origin-endpoint-startover-window-negative-rejected.

Cause 3 of 3: RotateIngestEndpointCredentials' NotFoundException message is not built from the request the way AWS's is — it omits the channel id.
AWS: recorded behavior only — Smithy (RotateIngestEndpointCredentials, PUT /channels/{Id}/ingest_endpoints/{IngestEndpointId}/credentials) models both Id and IngestEndpointId as required httpLabels and NotFoundException (httpError 404). The recorded response for an IngestEndpointId that does not belong to the channel is NotFoundException whose message is composed from BOTH request labels: "channel with id=<Id> does not have Ingest Endpoint with guid = <IngestEndpointId> " (AWS emits a trailing space after the guid). The rule is request-derived — the message names the channel the lookup was scoped to and the guid that was not found — so that rule, not the literal recorded text, is the requirement.
Emulator: answers the right code (NotFoundException) but a message scoped only to the endpoint id — "ingestEndpoint with id=not-a-real-ingest-endpoint-id not found" — dropping the channel that the lookup was scoped to.
Why: a client or operator diagnosing a rotation failure cannot tell from the emulator's message which channel the guid was looked up under, which is precisely the information AWS's message carries for this per-channel-scoped lookup; log scrapers and error-mapping code keyed on the channel-scoped form do not match.
Fix: the RotateIngestEndpointCredentials handler's not-found path — build the NotFoundException message from both path labels (channel Id and IngestEndpointId) instead of the generic single-resource not-found template. The rejected rotation correctly left the real credentials untouched, so only the message construction is at fault.
Traces: mediapackage.rotate-ingest-endpoint-credentials-invalid-endpoint-id-rejected.

Not part of this issue (checked and deliberately excluded): mediapackage.create-harvest-job-unassumable-role-forbidden and mediapackage.list-origin-endpoints-malformed-next-token-rejected fail only on canned error prose whose generating rule is neither modeled nor request-derived (codes and statuses matched). Five further failures are already owned by 20261022T213438Z-mediapackage-data-plane-hls-ingest-url-is-unrouted.md.

## Evidence
Cause 1 — mediapackage.harvest-job-list-filters-reflect-status:
  expected: ClientError UnprocessableEntityException (HTTP 422) for ListHarvestJobs(IncludeChannelId=<channel>, IncludeStatus="IN_PROGRESS")
  actual:   AssertionError: real AWS refuses this call, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'HarvestJobs': [{'Arn': 'arn:aws:mediapackage:us-west-2:...:harvest_jobs/sim2real-mp-hj-...', 'ChannelId': 'sim2real-mp-ch-...', 'Status': 'IN_PROGRESS', ...}]}

Cause 2 — mediapackage.origin-endpoint-startover-window-negative-rejected (CreateOriginEndpoint with StartoverWindowSeconds=-100):
  expected: UnprocessableEntityException, message 'startoverWindowSeconds must be greater than or equal to 300'
  actual:   UnprocessableEntityException, message 'startoverWindowSeconds must be greater than or equal to 0'
  => the enforced minimum is 0, so 1..299 are accepted where AWS rejects them

Cause 3 — mediapackage.rotate-ingest-endpoint-credentials-invalid-endpoint-id-rejected:
  expected: NotFoundException, message 'channel with id=<channelId> does not have Ingest Endpoint with guid = <ingestEndpointId> ' (both request labels, trailing space)
  actual:   NotFoundException, message 'ingestEndpoint with id=not-a-real-ingest-endpoint-id not found' (channel id absent)

## Steps to Reproduce
Cause 1:
1. mediapackage.create_channel(Id=<ch>); create_origin_endpoint(Id=<oe>, ChannelId=<ch>, HlsPackage={}, StartoverWindowSeconds=86400)
2. Create an S3 bucket + IAM role that trusts mediapackage.amazonaws.com, then create_harvest_job over a past window (job reports IN_PROGRESS).
3. Call list_harvest_jobs(IncludeChannelId=<ch>, IncludeStatus="IN_PROGRESS").
   Expected: ClientError UnprocessableEntityException / HTTP 422. Observed: HTTP 200 with the job list.

Cause 2:
1. mediapackage.create_channel(Id=<ch>)
2. mediapackage.create_origin_endpoint(Id=<oe>, ChannelId=<ch>, HlsPackage={}, StartoverWindowSeconds=-100) -> inspect the UnprocessableEntityException message: it states the bound as 0, not 300.
3. mediapackage.create_origin_endpoint(Id=<oe2>, ChannelId=<ch>, HlsPackage={}, StartoverWindowSeconds=60) -> the emulator accepts it; real MediaPackage rejects with UnprocessableEntityException.

Cause 3:
1. created = mediapackage.create_channel(Id=<ch>) and note created['HlsIngest']['IngestEndpoints'] ids.
2. mediapackage.rotate_ingest_endpoint_credentials(Id=<ch>, IngestEndpointId="not-a-real-ingest-endpoint-id")
   Expected: NotFoundException whose message names both <ch> and the bogus guid. Observed: a message naming only the guid.
