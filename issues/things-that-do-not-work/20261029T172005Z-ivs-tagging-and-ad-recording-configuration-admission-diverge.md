Issue-Source: trace-sweep-divergence
Issue-Trace: ivs-tagging-workflow
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ivs-tagging-workflow ivs-stream-key-management ivs-list-tags-for-resource-not-found ivs-ad-configuration-lifecycle-attempt ivs-recording-configuration-error-paths ivs-batch-get-channel-and-stream-key
Issue-Shape-Id: com.amazonaws.ivs#AmazonInteractiveVideoService
Issue-Key: ivs|TagResource|2xx->2xx|status
Issue-Key: ivs|UntagResource|2xx->2xx|status
Issue-Key: ivs|DeleteStreamKey|2xx->2xx|$headers.content-length
Issue-Key: ivs|ListTagsForResource|2xx->4xx|$.tags
Issue-Key: ivs|ListTagsForResource|2xx->4xx|status
Issue-Key: ivs|CreateAdConfiguration|4xx->2xx|$.Code
Issue-Key: ivs|CreateAdConfiguration|4xx->2xx|$.Message
Issue-Key: ivs|CreateAdConfiguration|4xx->2xx|$.RequestID
Issue-Key: ivs|CreateAdConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: ivs|CreateAdConfiguration|4xx->2xx|status
Issue-Key: ivs|ListAdConfigurations|2xx->2xx|$.adConfigurations[*]
Issue-Key: ivs|CreateRecordingConfiguration|4xx->2xx|$.Code
Issue-Key: ivs|CreateRecordingConfiguration|4xx->2xx|$.Message
Issue-Key: ivs|CreateRecordingConfiguration|4xx->2xx|$.RequestID
Issue-Key: ivs|CreateRecordingConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: ivs|CreateRecordingConfiguration|4xx->2xx|status
Issue-Key: ivs|BatchGetChannel|2xx->2xx|$.errors[*].message
Issue-Related: 20261029T172005Z-ivs-channel-and-recording-configuration-reads-omit-documented.md
Issue-Commit: 4fccfcb9e83e9e12ca223d9fc117bac8a0390f59

# IVS tagging and ad/recording-configuration admission diverge in status and shape (8 causes)

> _Note: this issue was identified against the code base at commit `4fccfcb9e83e9e12ca223d9fc117bac8a0390f59`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Eight net-new wire causes where the emulator answers a different HTTP status, admits a request AWS refuses, or words a per-item error differently. They share one service and one family of fault — the decision a call reaches and the status it is answered with — and each is a separate fixture step that must be closed.

Cause 1 of 8: TagResource answers 200 where AWS answers 204. Fixture ivs-tagging-workflow, step task#2. AWS: Smithy TagResource POST /tags/{resourceArn} -> TagResourceResponse, a structure with NO members; recorded behavior: the response is HTTP 204 with an empty body. Emulator: HTTP 200. Why: a client branching on 204 (or on "no content") for a successful tag write sees 200 and, worse, a success status the recording never uses. Fix: the operation's success status in its handler/serializer (SUCCESS_STATUS for TagResource).

Cause 2 of 8: UntagResource answers 200 where AWS answers 204. Fixture ivs-tagging-workflow, step task#4. AWS: Smithy UntagResource DELETE /tags/{resourceArn} -> UntagResourceResponse, no members; recorded behavior: HTTP 204, empty body. Emulator: HTTP 200. Why and Fix: as cause 1, for UntagResource.

Cause 3 of 8: DeleteStreamKey's 204 carries no content-length header. Fixture ivs-stream-key-management, step task#6. AWS: Smithy DeleteStreamKey POST /DeleteStreamKey with @http code 204 and a Unit output; recorded behavior: the response carries `content-length: 0`. Emulator: 204 with the header absent. Why: a client or proxy that frames the response from content-length has to fall back to connection semantics on an IVS delete. Fix: the empty-body response framing in the service's serializer. (Severity of this cause alone is LOW; the bundle takes the highest.)

Cause 4 of 8: ListTagsForResource answers 404 for a resource that exists and whose tags were accepted. Fixture ivs-list-tags-for-resource-not-found, step task#3. AWS: Smithy ListTagsForResource GET /tags/{resourceArn} -> tags, with ResourceNotFoundException reserved for an ARN that names nothing; recorded behavior: 200 with the resource's tag map (the probe's tags as written at create time). Emulator: 404 and no tags member, because crate::state::resource_tags/arn_resource_type resolve only `channel`, `stream-key` and `recording-configuration` ARNs (state.rs:1239-1270) and fall through to not-found for every other IVS resource type the service can now create — playback-key included. Why: tags set when the resource was created are unreadable, and a resource the client just created is reported as non-existent. Fix: the shared ARN -> resource resolution in crate::state, which must cover every resource type this service can create.

Cause 5 of 8: CreateAdConfiguration admits a mediaTailorPlaybackConfigurations entry naming a MediaTailor playback configuration that does not exist. Fixture ivs-ad-configuration-lifecycle-attempt, step task#1. AWS: Smithy CreateAdConfiguration declares ResourceNotFoundException (@httpError 404) and requires mediaTailorPlaybackConfigurations (1..3 entries, each a MediaTailorPlaybackConfiguration whose playbackConfigurationArn must name "the customer-created EMT PlaybackConfiguration resource in the same region and account"); recorded behavior: a well-formed ARN whose MediaTailor playback configuration does not exist is refused 404 ResourceNotFoundException with Message of the form `Resource: mediaTailorPlaybackConfiguration '<name>' not found` — fixed text around the playback configuration's name taken from the request's ARN — plus the x-amzn-errortype header naming the code. Emulator: HTTP 200, and the ad configuration is created. Why: a client that mistypes or forgets to create its MediaTailor playback configuration gets a working ad configuration locally and an SSAI setup that silently inserts nothing. Fix: validation in the CreateAdConfiguration handler — resolve each playbackConfigurationArn against the emulator's MediaTailor state before minting the configuration.

Cause 6 of 8: ListAdConfigurations then reports an ad configuration AWS never created. Fixture ivs-ad-configuration-lifecycle-attempt, step task#2. AWS: recorded behavior — after the refusal in cause 5 the account holds no such ad configuration, so no adConfigurations entry with that name is present. Emulator: the leaked configuration is listed in full (arn, name, mediaTailorPlaybackConfigurations, postRollConfiguration, tags). Why: state diverges for every later read, so a test asserting "the bad create left nothing behind" passes against AWS and fails here. Fix: the same admission check as cause 5 (nothing is persisted when the reference does not resolve); the list handler itself is correct.

Cause 7 of 8: CreateRecordingConfiguration admits a destinationConfiguration.s3 bucket that does not exist. Fixture ivs-recording-configuration-error-paths, step task#1. AWS: Smithy CreateRecordingConfiguration carries the service-common ValidationException (@httpError 400, with the x-amzn-ErrorType header); recorded behavior: a syntactically valid but non-existent/unowned bucket is refused synchronously with 400 ValidationException and nothing is created, message of the handler form `Request member: recordingConfiguration.destinationConfiguration.s3 failed to satisfy the constraints: recordingConfiguration.destinationConfiguration.s3 is invalid` (fixed text; the member path is the wire path). Emulator: HTTP 200 (and, per the empty-envelope defect, no Code/Message/x-amzn-errortype would be carried even once it refuses). Why: recordings are silently lost instead of the up-front rejection AWS gives. Fix: validation in the CreateRecordingConfiguration handler — resolve the bucket against the emulator's S3 state before minting the configuration. This cause is ALSO described as Cause 4 of the open behavioral issue 20261023T004057Z-ivs-7-behavioural-divergences-error-messages-dropped-on.md, which carries no replay key for this fixture step; it is filed here so the wire step is re-replayed.

Cause 8 of 8: BatchGetChannel's per-item error message is the emulator's own wording. Fixture ivs-batch-get-channel-and-stream-key, step task#4. AWS: Smithy BatchGetChannel returns errors[] of BatchError{arn, code, message}; recorded behavior: for an ARN naming no channel the item's message is the service's one not-found sentence — fixed text `Resource: ` + the requested ARN + ` not found`. Emulator: `channel <arn> does not exist`. Why: a client reading errors[].message gets prose that does not match the service's documented not-found wording, so text-matching and user-facing messages diverge from AWS. Fix: the BatchGetChannel handler must build the per-item message from the shared not-found message rule in crate::state (resource_not_found_message) instead of its own string.

Note on values excluded as per-run: the same BatchGetChannel cluster also reports ingestEndpoint / srt.endpoint host differences and the missing channel members covered by the projection bundle; those are not this issue.

## Evidence
- TagResource ivs-tagging-workflow#task#2: status expected 204, got 200.
- UntagResource ivs-tagging-workflow#task#4: status expected 204, got 200.
- DeleteStreamKey ivs-stream-key-management#task#6: status 204 -> 204, $headers.content-length expected '0' got None.
- ListTagsForResource ivs-list-tags-for-resource-not-found#task#3: status expected 200, got 404; $.tags expected {'probe': 'sim2real'} got None.
- CreateAdConfiguration ivs-ad-configuration-lifecycle-attempt#task#1: status expected 404, got 200; $.Code expected 'ResourceNotFoundException' got None; $.Message expected "Resource: mediaTailorPlaybackConfiguration 'probe-config' not found" got None; $headers.x-amzn-errortype expected (shaped) got None. 'probe-config' is the run's playback-configuration name, taken from the request.
- ListAdConfigurations ivs-ad-configuration-lifecycle-attempt#task#2: $.adConfigurations[0] unexpected_present — no entry named <probe name> expected, got the full created object {arn, name, mediaTailorPlaybackConfigurations, postRollConfiguration {durationSeconds 15, enabled false}, tags}.
- CreateRecordingConfiguration ivs-recording-configuration-error-paths#task#1: status expected 400, got 200; $.Code expected 'ValidationException' got None; $.Message expected 'Request member: recordingConfiguration.destinationConfiguration.s3 failed to satisfy the constraints: recordingConfiguration.destinationConfiguration.s3 is invalid' got None; $headers.x-amzn-errortype expected 'ValidationException' got None.
- BatchGetChannel ivs-batch-get-channel-and-stream-key#task#4: $.errors[0].message expected 'Resource: arn:aws:ivs:us-west-2:000000000000:channel/does-not-exist not found', got 'channel arn:aws:ivs:us-west-2:000000000000:channel/does-not-exist does not exist'.

## Steps to Reproduce
Boot the emulator from this checkout, region us-west-2:
1. CreateChannel, then POST /tags/<channel arn> with {"tags":{"probe":"sim2real"}} -> 200 instead of 204; DELETE /tags/<channel arn>?tagKeys=probe -> 200 instead of 204.
2. CreateStreamKey then POST /DeleteStreamKey -> 204 without a content-length header.
3. ImportPlaybackKeyPair with tags {"probe":"sim2real"}, then GET /tags/<keyPair arn> -> 404 instead of 200 with the tags (fixture ivs-list-tags-for-resource-not-found#task#3).
4. POST /CreateAdConfiguration {"name":...,"mediaTailorPlaybackConfigurations":[{"playbackConfigurationArn":"arn:aws:mediatailor:us-west-2:000000000000:playbackConfiguration/probe-config"}]} with no such MediaTailor playback configuration -> 200 instead of 404; then POST /ListAdConfigurations -> the configuration is listed.
5. POST /CreateRecordingConfiguration with destinationConfiguration.s3.bucketName naming a bucket that was never created -> 200 instead of 400 ValidationException.
6. POST /BatchGetChannel {"arns":[<a real channel arn>, "arn:aws:ivs:us-west-2:000000000000:channel/does-not-exist"]} -> errors[0].message is 'channel <arn> does not exist' instead of 'Resource: <arn> not found'.
