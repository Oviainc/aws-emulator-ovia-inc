Issue-Source: trace-sweep-divergence
Issue-Trace: mediapackagev2-harvest-job-lifecycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediapackagev2-harvest-job-lifecycle mediapackagev2-channel-policy-lifecycle mediapackagev2-origin-endpoint-policy-lifecycle mediapackagev2-error-paths mediapackagev2-tagging-workflow
Issue-Shape-Id: com.amazonaws.mediapackagev2#mediapackagev2
Issue-Key: mediapackagev2|CreateHarvestJob|4xx->2xx|$.Message
Issue-Key: mediapackagev2|CreateHarvestJob|4xx->2xx|$.ValidationExceptionType
Issue-Key: mediapackagev2|CreateHarvestJob|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: mediapackagev2|CreateHarvestJob|4xx->2xx|status
Issue-Key: mediapackagev2|ListHarvestJobs|2xx->2xx|$.Items[*]
Issue-Key: mediapackagev2|GetChannelPolicy|2xx->2xx|$.Policy
Issue-Key: mediapackagev2|GetOriginEndpointPolicy|2xx->2xx|$.Policy
Issue-Key: mediapackagev2|CreateChannelGroup|2xx->2xx|$.ETag
Issue-Key: mediapackagev2|CreateChannel|2xx->2xx|$.ETag
Issue-Key: mediapackagev2|CreateChannel|2xx->2xx|$.IngestEndpoints[*].Url
Issue-Key: mediapackagev2|TagResource|2xx->2xx|$headers.content-length
Issue-Key: mediapackagev2|UntagResource|2xx->2xx|$headers.content-length
Issue-Commit: adba6aa61f0ba2f78a6b2d7c8f4b487f05884eaf

# mediapackagev2 emulator diverges from recorded AWS responses: missing harvest-job S3 validation, policy/ETag/ingest/header serialization

> _Note: this issue was identified against the code base at commit `adba6aa61f0ba2f78a6b2d7c8f4b487f05884eaf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateHarvestJob

## Description
Replay of the mediapackagev2 wire fixtures (86/96 steps passed) surfaced 5 distinct emulator defects in how mediapackagev2 responses are produced. They are bundled because they share one service; a fixer MUST close ALL FIVE causes below before submitting.

Cause 1 of 5: CreateHarvestJob does not validate the S3 harvest destination and accepts an unwritable bucket.
  AWS: CreateHarvestJob models ValidationException (client error, httpError 400, Message + ValidationExceptionType) and the ValidationExceptionType enum includes the member HARVEST_JOB_UNABLE_TO_WRITE_TO_S3_DESTINATION (Smithy: CreateHarvestJob errors -> ValidationException @httpError 400; shape ValidationException members Message, ValidationExceptionType; enum ValidationExceptionType value HARVEST_JOB_UNABLE_TO_WRITE_TO_S3_DESTINATION). The recorded response for a Destination.S3Destination whose bucket MediaPackageV2 cannot write to is HTTP 400 with header x-amzn-errortype: ValidationException, ValidationExceptionType: HARVEST_JOB_UNABLE_TO_WRITE_TO_S3_DESTINATION, and a Message naming the offending member (example text: "Validation error encountered at request.s3DestinationConfig: Unable to write to S3 destination bucket. Verify MediaPackageV2 service principal mediapackagev2.amazonaws.com is allow listed write access to the bucket and the bucket region is in the same region."). The requirement is the 400 status, the x-amzn-errortype: ValidationException header and the ValidationExceptionType value; the exact sentence is an example.
  Emulator: returns HTTP 200 and creates the job. No Message, no ValidationExceptionType, no x-amzn-errortype header is emitted.
  Why: a client that depends on the create call rejecting an unreachable destination gets a QUEUED job that can never succeed, and error-handling paths keyed on ValidationException/HARVEST_JOB_UNABLE_TO_WRITE_TO_S3_DESTINATION are never exercised. The phantom job then leaks into subsequent reads: the very next recorded step, ListHarvestJobs, expects an empty Items list and the emulator returns the job that should never have been created (fixture mediapackagev2-harvest-job-lifecycle step task#5, $.Items[0] extra). Fixing the validation is expected to fix that step too.
  Fix: CreateHarvestJob's validation (destination/S3 reachability precondition) plus the ValidationException serializer so the modeled ValidationExceptionType member and the x-amzn-errortype header are emitted.
  Traces: mediapackagev2-harvest-job-lifecycle#task#4 (400 expected, 200 returned), mediapackagev2-harvest-job-lifecycle#task#5 (ListHarvestJobs shows the leaked job).

Cause 2 of 5: resource policy documents are re-serialized instead of returned as stored.
  AWS: GetChannelPolicy/GetOriginEndpointPolicy return the Policy member as a PolicyText string (Smithy: GetChannelPolicy.output.Policy -> PolicyText, required, @length max 6144 — an opaque string, not a structured document). Recorded behavior: the policy document is returned with the exact JSON text layout AWS stored for it (pretty-printed, keys separated by " : ", statements on their own lines).
  Emulator: returns a re-encoded, compact single-line JSON object ({"Version": "2012-10-17", "Statement": [...]}) — semantically equal, textually different, i.e. the submitted document text is not preserved through Put -> Get.
  Why: PolicyText is an opaque string on the wire, so clients that hash, diff, or byte-compare the policy they put against the policy they get back (drift detection, idempotent re-apply, CloudFormation-style change detection) see a spurious change on every read.
  Fix: the Put*Policy state write / Get*Policy serializer — store and return the policy string verbatim rather than parsing and re-dumping it.
  Traces: mediapackagev2-channel-policy-lifecycle#task#5 (GetChannelPolicy, $.Policy value_mismatch), mediapackagev2-origin-endpoint-policy-lifecycle#task#6 (GetOriginEndpointPolicy, $.Policy value_mismatch).

Cause 3 of 5: ETag is emitted as a 64-character hex digest instead of a base64-encoded digest.
  AWS: ETag targets EntityTag (Smithy: shape EntityTag — string, @length 1..256, @pattern ^[\\S]+$) and is returned by CreateChannelGroup/CreateChannel/CreateHarvestJob/Get*. Recorded behavior only: every recorded ETag is a 44-character base64 encoding of a 32-byte digest, ending in '=' (examples: '3hio0YfHaGSwHNsw8GIjKInIOXUh6FS4NRJ599WB2iY=', 'PSu2M7rhYnHflghGNlu5mX0IEPstgN/wdWUYFf6TS/c=').
  Emulator: emits the digest hex-encoded, 64 lowercase hex characters (examples: 'c8775549bbf08198e514a2750f0e8db79baf6892721ad7e1adc9ee07ecc489c2', '5244052cff45ea799b5940a59f9e492d901779bd24483f9bb4289ce680845395'). The particular digest value is content/environment dependent and is NOT the requirement; the encoding and resulting token shape are.
  Why: ETags are echoed back by clients in x-amzn-update-if-match and are commonly persisted or validated for shape; an emulator that issues hex tokens where AWS issues base64 ones makes recorded/real tokens non-interchangeable and breaks any client that round-trips or pattern-checks them.
  Fix: the shared ETag computation helper used by the mediapackagev2 handlers/state — base64-encode the 32-byte digest instead of hex-encoding it.
  Traces: mediapackagev2-error-paths#task#5 (CreateChannelGroup, $.ETag), mediapackagev2-error-paths#task#7 (CreateChannel, $.ETag).

Cause 4 of 5: CreateChannel returns the same ingest hostname for every ingest endpoint.
  AWS: CreateChannel.output.IngestEndpoints is an IngestEndpointList of IngestEndpoint{Id, Url} (Smithy: shape IngestEndpoint, member Url doc "The ingest domain URL where the source stream should be sent"). Recorded behavior only: the two ingest endpoints of a channel are served on DISTINCT ingest hostnames carrying a per-endpoint ordinal (recorded example hosts 'cq8go8-1.ingest.z4xeig.mediapackagev2.us-west-2.amazonaws.com' and 'cq8go8-2.ingest.z4xeig.mediapackagev2.us-west-2.amazonaws.com'), differing only in the ordinal suffix of the first label.
  Emulator: emits one identical host for both endpoints ('096530b1.ingest.4a962433403d.mediapackagev2.us-west-2.amazonaws.com'), distinguishing the endpoints only by URL path segment. The random label text and the channel-group egress-domain label are generated per environment and are NOT requirements; the per-endpoint distinctness of the ingest host is the recorded behavior that diverges.
  Why: a client that pins or load-balances per ingest endpoint by host (two publishers sending to the two redundant ingest endpoints) cannot distinguish them, and any assertion that the two endpoints are separate ingest targets fails.
  Fix: the CreateChannel handler / channel state that materializes IngestEndpoints — derive one host per endpoint with its ordinal, rather than reusing a single channel-level host.
  Traces: mediapackagev2-error-paths#task#7 ($.IngestEndpoints[0].Url and $.IngestEndpoints[1].Url).

Cause 5 of 5: empty 204 responses omit the Content-Length: 0 header.
  AWS: TagResource and UntagResource have no output members (Smithy: TagResource output Unit, @http code 204; UntagResource output Unit, @http code 204, idempotent). Recorded behavior: the 204 response carries content-length: 0.
  Emulator: returns 204 with no content-length header at all (header missing, expected '0').
  Why: clients and intermediaries that read framing headers to decide there is no body (or that require an explicit zero length on a non-chunked response) see a differently framed response than AWS sends; any wire-level assertion on the tagging calls fails even though the status is right.
  Fix: the REST-JSON response serializer for empty-output operations — set Content-Length: 0 on bodiless 204 responses.
  Traces: mediapackagev2-tagging-workflow#task#3 (TagResource), mediapackagev2-tagging-workflow#task#5 (UntagResource).

Not included: the GetHarvestJob/CancelHarvestJob not-found responses (mediapackagev2-harvest-job-lifecycle task#6/task#7) match AWS on status (404) and differ only in the wording of the human-readable Message, whose generating rule is neither modeled nor documented, so that is not filed as a defect. The one open mediapackagev2 issue (20261022T023331Z-mediapackagev2-listtagsforresource-stub.md, ListTagsForResource stub) covers a different operation and none of the causes above.

## Evidence
Replay summary: trace_set_id=simparity-wire-aws-v1, emulator_up=true, steps_total=96, passed=86, mismatched=10, state_reset_failures=0, bootstrap_signal=false, score_pct=89.6.

Cause 1 — mediapackagev2-harvest-job-lifecycle#task#4 CreateHarvestJob:
  expected: status 400, $.Message 'Validation error encountered at request.s3DestinationConfig: Unable to write to S3 destination bucket. Verify MediaPackageV2 service principal mediapackagev2.amazonaws.com is allow listed write access to the bucket and the bucket region is in the same region.', $.ValidationExceptionType 'HARVEST_JOB_UNABLE_TO_WRITE_TO_S3_DESTINATION', header x-amzn-errortype 'ValidationException'
  actual:   status 200, all three absent (None)
  follow-on mediapackagev2-harvest-job-lifecycle#task#5 ListHarvestJobs: expected $.Items[0] absent; actual $.Items[0] = the harvest job for bucket 'sim2real-probe-harvest-bucket-unwritable' with Status 'QUEUED'.

Cause 2 — mediapackagev2-channel-policy-lifecycle#task#5 GetChannelPolicy (status 200 both sides):
  expected $.Policy: pretty-printed document, e.g. '{\n  "Version" : "2012-10-17",\n  "Statement" : [ {\n    "Sid" : "AllowAccountRootIngest", ... } ]\n}'
  actual   $.Policy: '{"Version": "2012-10-17", "Statement": [{"Sid": "AllowAccountRootIngest", ...}]}'
  mediapackagev2-origin-endpoint-policy-lifecycle#task#6 GetOriginEndpointPolicy: same pattern with Sid 'AllowAccountRoot' / Action 'mediapackagev2:GetObject'.

Cause 3 — mediapackagev2-error-paths#task#5 CreateChannelGroup (status 200 both sides):
  expected $.ETag '3hio0YfHaGSwHNsw8GIjKInIOXUh6FS4NRJ599WB2iY=' (44 chars, base64)
  actual   $.ETag 'c8775549bbf08198e514a2750f0e8db79baf6892721ad7e1adc9ee07ecc489c2' (64 chars, hex)
  mediapackagev2-error-paths#task#7 CreateChannel: expected 'PSu2M7rhYnHflghGNlu5mX0IEPstgN/wdWUYFf6TS/c=', actual '5244052cff45ea799b5940a59f9e492d901779bd24483f9bb4289ce680845395'.

Cause 4 — mediapackagev2-error-paths#task#7 CreateChannel (status 200 both sides):
  expected $.IngestEndpoints[0].Url host 'cq8go8-1.ingest.z4xeig.mediapackagev2.us-west-2.amazonaws.com', [1].Url host 'cq8go8-2.ingest.z4xeig...' (distinct hosts, ordinal suffix)
  actual   both URLs on host '096530b1.ingest.4a962433403d.mediapackagev2.us-west-2.amazonaws.com' (identical host for both endpoints)

Cause 5 — mediapackagev2-tagging-workflow#task#3 TagResource and #task#5 UntagResource (status 204 both sides):
  expected header content-length '0'; actual header missing (None).

## Steps to Reproduce
Boot the emulator from this checkout and replay the mediapackagev2 wire fixtures (replay_wire_corpus against trace set simparity-wire-aws-v1).

Cause 1: fixture mediapackagev2-harvest-job-lifecycle, step task#4 — POST /channelGroup/{g}/channel/{c}/originEndpoint/{e}/harvestJob with Destination.S3Destination.BucketName pointing at a bucket MediaPackageV2 cannot write to ('sim2real-probe-harvest-bucket-unwritable'). Expect 400 + x-amzn-errortype: ValidationException + ValidationExceptionType HARVEST_JOB_UNABLE_TO_WRITE_TO_S3_DESTINATION; observe 200 and a created job. Then step task#5 — GET /channelGroup/{g}/harvestJob: expect no Items, observe the leaked QUEUED job.

Cause 2: fixture mediapackagev2-channel-policy-lifecycle, step task#5 — PutChannelPolicy with a multi-line policy document, then GET /channelGroup/{g}/channel/{c}/policy and byte-compare $.Policy with what was submitted. Same with fixture mediapackagev2-origin-endpoint-policy-lifecycle step task#6 (GetOriginEndpointPolicy).

Cause 3: fixture mediapackagev2-error-paths, step task#5 (POST /channelGroup) and step task#7 (POST /channelGroup/{g}/channel) — inspect $.ETag: 64 hex chars instead of a 44-char base64 token.

Cause 4: fixture mediapackagev2-error-paths, step task#7 — inspect $.IngestEndpoints[*].Url and compare the hostnames of the two endpoints; they are identical.

Cause 5: fixture mediapackagev2-tagging-workflow, step task#3 (POST /tags/{arn}) and step task#5 (DELETE /tags/{arn}?tagKeys=...) — inspect the 204 response headers for content-length: 0.
