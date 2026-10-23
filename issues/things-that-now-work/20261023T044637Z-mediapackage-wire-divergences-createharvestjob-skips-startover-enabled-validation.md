Issue-Source: trace-sweep-divergence
Issue-Trace: mediapackage-harvest-job-workflow
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediapackage-harvest-job-workflow mediapackage-channel-lifecycle-tags-logs mediapackage-origin-endpoint-lifecycle
Issue-Shape-Id: com.amazonaws.mediapackage#MediaPackage
Issue-Key: mediapackage|CreateHarvestJob|4xx->2xx|$.errors
Issue-Key: mediapackage|CreateHarvestJob|4xx->2xx|$.message
Issue-Key: mediapackage|CreateHarvestJob|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: mediapackage|CreateHarvestJob|4xx->2xx|status
Issue-Key: mediapackage|ListHarvestJobs|2xx->2xx|$.harvestJobs[*]
Issue-Key: mediapackage|TagResource|2xx->2xx|$headers.content-length
Issue-Key: mediapackage|UntagResource|2xx->2xx|$headers.content-length
Issue-Commit: 134aa7970ed0c23d5be465e9c36f71c1d4a91758

# MediaPackage wire divergences: CreateHarvestJob skips startover-enabled validation and 204 tag responses omit Content-Length (4 causes)

> _Note: this issue was identified against the code base at commit `134aa7970ed0c23d5be465e9c36f71c1d4a91758`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateHarvestJob, ListHarvestJobs, TagResource, UntagResource

## Description
Four wire-fixture divergences in the mediapackage emulator (crate services/service-com-amazonaws-mediapackage-mediapackage), found by replaying trace set simparity-wire-aws-v1. Causes 1-2 are a missing CreateHarvestJob precondition and the state it leaks into ListHarvestJobs; causes 3-4 are the 204 no-content responses of the tagging operations missing their framing header. A fixer must close ALL FOUR and re-replay every fixture/step named below, not just the representative step in the front matter.

Cause 1 of 4: CreateHarvestJob accepts an OriginEndpointId whose origin endpoint does not have startover enabled, instead of refusing it with UnprocessableEntityException/422. (fixture mediapackage-harvest-job-workflow, step task#3)
AWS: Smithy: CreateHarvestJob = POST /harvest_jobs, success code 200, input requires Id/OriginEndpointId/StartTime/EndTime/S3Destination; it models UnprocessableEntityException with httpError 422 (shape UnprocessableEntityException, single member Message/jsonName "message", doc "The parameters sent in the request are not valid."). The model's doc for CreateHarvestJobRequest$OriginEndpointId ("The ID of the OriginEndpoint that the HarvestJob will harvest from. This cannot be changed after the HarvestJob is submitted.") states no startover precondition, so the rule itself is recorded behavior only: the recording shows that harvesting is only accepted from an endpoint with a startover window, and a create naming an endpoint without one is refused. Recorded response: HTTP 422 with header x-amzn-errortype: UnprocessableEntityException and a body carrying message "originEndpointId must have startover enabled" plus errors {"originEndpointId": ["must have startover enabled"]}. The requirements are the 422 status and the UnprocessableEntityException error type; the message and the errors detail map are examples of a request-derived validation message naming the offending member and its unmet constraint, not byte requirements.
Emulator: the same request answers HTTP 200 with a success CreateHarvestJob body and sends no x-amzn-errortype header, no message and no errors member — the precondition is not checked at all, and the job is persisted.
Why: a client that points a harvest job at a non-startover endpoint is told the job was accepted. On AWS it gets a 422 it can surface and correct; against the emulator it waits on a harvest that can never produce output, and any client branching on UnprocessableEntityException for the whole input-validation class never sees that branch.
Fix: the CreateHarvestJob handler's input validation in the mediapackage crate: resolve OriginEndpointId against origin-endpoint state and require a startover window to be configured before accepting, answering UnprocessableEntityException/422 and persisting nothing otherwise.
Related-but-not-covering: open issue "MediaPackage control plane: negative-class rejections missing — StartoverWindowSeconds<0 accepted, CreateHarvestJob validations and Rotate*Credentials ..." covers CreateHarvestJob's time-range and unassumable-role rejections and was written when the operation still answered NotImplemented; it does not name the startover precondition and carries no wire replay key for this step, so this cause is net-new.
Traces: mediapackage-harvest-job-workflow#task#3

Cause 2 of 4: after that refused create, ListHarvestJobs still reports the harvest job, so the rejected create leaked state. (fixture mediapackage-harvest-job-workflow, step task#4)
AWS: Smithy: ListHarvestJobs = GET /harvest_jobs, output HarvestJobs (jsonName "harvestJobs", list of HarvestJob) + NextToken, paginated. Recorded behavior: because the preceding CreateHarvestJob was refused, the listing contains NO harvestJobs entry for that id — a create that fails validation persists nothing.
Emulator: HTTP 200 whose body carries an extra harvestJobs[0] entry for the id that should not exist, with status IN_PROGRESS and the arn/channelId/originEndpointId/s3Destination of the rejected request (ids, arns and timestamps in the recording are environment-specific examples; the requirement is the absence of the entry).
Why: a client that polls ListHarvestJobs/DescribeHarvestJob after a failed submit sees a phantom IN_PROGRESS job, so it cannot tell a rejected submission from an accepted one, and cleanup/idempotency logic keyed on "does this job exist" takes the wrong branch.
Fix: the same CreateHarvestJob validation path plus harvest-job state in the mediapackage crate — reject before inserting into state so the listing stays empty. This cause is downstream of cause 1, but it is a separate replayed step and MUST be re-replayed to confirm nothing was persisted.
Traces: mediapackage-harvest-job-workflow#task#4

Cause 3 of 4: TagResource's 204 No Content response omits the Content-Length header. (fixture mediapackage-channel-lifecycle-tags-logs, step task#6; also mediapackage-origin-endpoint-lifecycle#task#7)
AWS: Smithy: TagResource = POST /tags/{ResourceArn}, http code 204, output Unit (no modeled body members), no modeled errors. Recorded behavior: the response is 204 and carries a Content-Length header (the recording asserts the header is present, not its value).
Emulator: answers 204 with the header entirely absent.
Why: the response framing differs from AWS's. Clients, proxies and SDK transports that key on the presence of a length header for a bodyless response (or that distinguish "no body" from "body not yet framed") read the emulator's reply differently from the real service, which is exactly the class of difference a wire-fidelity emulator exists to remove.
Fix: the empty/no-content response serializer shared by the mediapackage tagging operations — emit the framing header for 204 Unit outputs.
Traces: mediapackage-channel-lifecycle-tags-logs#task#6, mediapackage-origin-endpoint-lifecycle#task#7

Cause 4 of 4: UntagResource's 204 No Content response omits the Content-Length header, the same way. (fixture mediapackage-channel-lifecycle-tags-logs, step task#8; also mediapackage-origin-endpoint-lifecycle#task#9)
AWS: Smithy: UntagResource = DELETE /tags/{ResourceArn} with required ResourceArn (httpLabel) and TagKeys (httpQuery "tagKeys"), http code 204, output Unit, no modeled errors. Recorded behavior: 204 with a Content-Length header present.
Emulator: 204 with no Content-Length header.
Why: same as cause 3 — bodyless responses are framed differently from AWS, so a client that inspects response headers behaves differently against the emulator than against the service.
Fix: same empty-response serializer path as cause 3; listed separately because it is a distinct operation and a distinct replayed step that must be re-replayed.
Traces: mediapackage-channel-lifecycle-tags-logs#task#8, mediapackage-origin-endpoint-lifecycle#task#9

## Evidence
Replay of wire trace set simparity-wire-aws-v1 against the emulator built from this checkout: 7 fixtures replayed, 63 steps, 56 passed, 7 mismatched, 0 errored, 0 blocked, score 88.9%, state_reset_failures 0, bootstrap_signal false (emulator_up: /__admin/health answered 200). 4 divergence clusters, all service=mediapackage, none carrying stub_path or owned_by.

Cause 1 — mediapackage-harvest-job-workflow#task#3, CreateHarvestJob, verdict mismatch:
  expected: status 422; $headers.x-amzn-errortype = 'UnprocessableEntityException'; $.message = 'originEndpointId must have startover enabled'; $.errors = {'originEndpointId': ['must have startover enabled']}
  actual:   status 200; x-amzn-errortype missing; $.message missing; $.errors missing

Cause 2 — mediapackage-harvest-job-workflow#task#4, ListHarvestJobs, verdict mismatch (expected_status 200, actual_status 200):
  expected: no $.harvestJobs[0]
  actual:   $.harvestJobs[0] extra = {'arn': 'arn:aws:mediapackage:us-west-2:000000000000:harvest_jobs/sim2real-probe-mediapackage-mp-harvest-d16d6f7d', 'channelId': 'sim2real-probe-mediapackage-mp-channel-d16d6f7d', 'id': 'sim2real-probe-mediapackage-mp-harvest-d16d6f7d', 'originEndpointId': 'sim2real-probe-mediapackage-mp-endpoint-d16d6f7d', 's3Destination': {...}, 'status': 'IN_PROGRESS', ...} (generated ids/arns/timestamps are environment-specific)

Cause 3 — mediapackage-channel-lifecycle-tags-logs#task#6, TagResource, verdict mismatch (expected_status 204, actual_status 204):
  detail: $headers.content-length: missing (expected '<shaped-not-asserted>', got None)
  cluster also affects mediapackage-origin-endpoint-lifecycle#task#7 (affected_count 2)

Cause 4 — mediapackage-channel-lifecycle-tags-logs#task#8, UntagResource, verdict mismatch (expected_status 204, actual_status 204):
  detail: $headers.content-length: missing (expected '<shaped-not-asserted>', got None)
  cluster also affects mediapackage-origin-endpoint-lifecycle#task#9 (affected_count 2)

Model evidence: smithy_query(query="operation", operation="CreateHarvestJob,ListHarvestJobs,TagResource,UntagResource") and smithy_query(query="shape", shape="UnprocessableEntityException") on services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json: CreateHarvestJob POST /harvest_jobs code 200 with errors [ForbiddenException 403, InternalServerErrorException 500, NotFoundException 404, ServiceUnavailableException 503, TooManyRequestsException 429, UnprocessableEntityException 422]; ListHarvestJobs GET /harvest_jobs, output harvestJobs/nextToken, paginated; TagResource POST /tags/{ResourceArn} code 204 output Unit; UntagResource DELETE /tags/{ResourceArn} code 204 output Unit; UnprocessableEntityException is a client error with httpError 422 and a single "message" member.

## Steps to Reproduce
1. git clone the repo (mainline) and build/boot the emulator from the checkout.
2. Replay the wire corpus: replay_wire_corpus() over trace set simparity-wire-aws-v1. All four clusters below reproduce deterministically.
3. Cause 1: replay fixture mediapackage-harvest-job-workflow, step task#3 — it creates a channel and an origin endpoint WITHOUT a startover window, then POSTs /harvest_jobs naming that endpoint. Observe HTTP 200 and a success body; AWS answers 422 with x-amzn-errortype: UnprocessableEntityException and a validation message naming originEndpointId.
4. Cause 2: continue to step task#4 of the same fixture — GET /harvest_jobs. Observe a harvestJobs entry for the job of step 3 (status IN_PROGRESS); the recording has no such entry because the create was refused.
5. Cause 3: replay fixture mediapackage-channel-lifecycle-tags-logs, step task#6 — POST /tags/{ResourceArn} on the channel arn. Observe 204 with NO Content-Length response header; the recording's 204 carries one. Same cluster: mediapackage-origin-endpoint-lifecycle step task#7.
6. Cause 4: same fixture, step task#8 — DELETE /tags/{ResourceArn}?tagKeys=... Observe 204 with NO Content-Length response header. Same cluster: mediapackage-origin-endpoint-lifecycle step task#9.
7. After fixing, re-replay all of: mediapackage-harvest-job-workflow#task#3, mediapackage-harvest-job-workflow#task#4, mediapackage-channel-lifecycle-tags-logs#task#6, mediapackage-channel-lifecycle-tags-logs#task#8 (plus the cluster members mediapackage-origin-endpoint-lifecycle#task#7 and #task#9).
