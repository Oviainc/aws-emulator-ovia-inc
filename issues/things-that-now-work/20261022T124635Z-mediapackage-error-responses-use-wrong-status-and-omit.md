Issue-Source: trace-sweep-divergence
Issue-Trace: mediapackage-listing-and-validation
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediapackage-listing-and-validation mediapackage-not-found-error-paths mediapackage-channel-lifecycle-tags-logs mediapackage-credential-rotation mediapackage-origin-endpoint-lifecycle mediapackage-conflict-error-paths
Issue-Shape-Id: com.amazonaws.mediapackage#MediaPackage
Issue-Key: mediapackage|ListChannels|4xx->4xx|$.errors
Issue-Key: mediapackage|ListChannels|4xx->4xx|$.message
Issue-Key: mediapackage|ListChannels|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: mediapackage|ListChannels|4xx->4xx|status
Issue-Key: mediapackage|CreateOriginEndpoint|4xx->4xx|$.errors
Issue-Key: mediapackage|CreateOriginEndpoint|4xx->4xx|$.message
Issue-Key: mediapackage|CreateOriginEndpoint|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: mediapackage|CreateOriginEndpoint|4xx->4xx|status
Issue-Key: mediapackage|DescribeChannel|4xx->4xx|$.errors
Issue-Key: mediapackage|DescribeChannel|4xx->4xx|$.message
Issue-Key: mediapackage|DeleteChannel|4xx->4xx|$.errors
Issue-Key: mediapackage|DeleteChannel|4xx->4xx|$.message
Issue-Key: mediapackage|DescribeOriginEndpoint|4xx->4xx|$.errors
Issue-Key: mediapackage|DescribeOriginEndpoint|4xx->4xx|$.message
Issue-Key: mediapackage|DeleteOriginEndpoint|4xx->4xx|$.errors
Issue-Key: mediapackage|DeleteOriginEndpoint|4xx->4xx|$.message
Issue-Key: mediapackage|CreateChannel|4xx->4xx|$.errors
Issue-Key: mediapackage|CreateChannel|4xx->4xx|$.message
Issue-Commit: 9979b14c0e509985792db6dac0171a1eff5d6a10

# MediaPackage error responses use wrong status and omit the errors member

> _Note: this issue was identified against the code base at commit `9979b14c0e509985792db6dac0171a1eff5d6a10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListChannels

## Description
This issue bundles 8 causes that share ONE fault: the emulator's MediaPackage error path does not produce the service's modeled error envelope - two causes return the wrong HTTP status and x-amzn-errortype entirely, and six return the right status but a body missing the 'errors' member that every recorded MediaPackage error carries. A fixer must close all 8 causes below before submitting.

Cause 1 of 8 - ListChannels maxResults out of range answers 400 ValidationException instead of 422 UnprocessableEntityException (fixture mediapackage-listing-and-validation, step task#5).
AWS: Smithy: ListChannels input MaxResults is @httpQuery('maxResults') with @range min 1 max 1000; the operation's modeled client errors are ForbiddenException(403), NotFoundException(404), TooManyRequestsException(429) and UnprocessableEntityException(422) - ValidationException is not an error of this service at all. The recording shows an out-of-range maxResults rejected with HTTP 422 (recorded message, as an example: 'maxResults must be an integer between 1 and 50' - the recorded bound is narrower than the modeled @range, so treat the recording as authoritative for the bound and Hypothesis: the service enforces a 50-item page cap on top of the modeled range).
Emulator: HTTP 400 with x-amzn-errortype 'ValidationException' and a generic smithy constraint message ('... failed to satisfy constraint: Member must have value less than or equal to 1000'), and no 'errors' member.
Why: the status class and error code are the contract a client switches on. An SDK sees an unmodeled ValidationException at 400, so retry/err-handling code written for UnprocessableEntityException never matches, and the exception does not even deserialize into a modeled type.
Fix: ListChannels' input validation must reject out-of-range maxResults as UnprocessableEntityException (422) using the service's own bound, not the generic constraint validator.
Traces: mediapackage-listing-and-validation#task#5.

Cause 2 of 8 - CreateOriginEndpoint against a missing channel answers 404 NotFoundException instead of 422 UnprocessableEntityException (fixture mediapackage-not-found-error-paths, step task#5).
AWS: Smithy: CreateOriginEndpointRequest.ChannelId is @required; the operation models both NotFoundException(404) and UnprocessableEntityException(422). The recording shows that an unknown channelId on CREATE is a request-validation failure: HTTP 422, x-amzn-errortype 'UnprocessableEntityException', errors {'channelId': ['not found']} (recorded message example: 'channelId not found').
Emulator: HTTP 404 with x-amzn-errortype 'NotFoundException' and a channel-not-found message.
Why: a client distinguishing 'the endpoint I asked about is gone' (404) from 'my request body is invalid' (422) takes the wrong branch - for example retrying/creating the parent channel versus surfacing a validation error to the user.
Fix: the CreateOriginEndpoint handler must map an unresolvable channelId to UnprocessableEntityException (422) with the member-scoped error, instead of reusing the lookup's NotFoundException.
Traces: mediapackage-not-found-error-paths#task#5.

Cause 3 of 8 - DescribeChannel 404 body omits the 'errors' member (fixture mediapackage-channel-lifecycle-tags-logs, step teardown#13).
AWS: Smithy: NotFoundException @httpError 404 with member Message (jsonName 'message'); the model declares nothing else, so recorded behavior only - the recorded 404 body also carries an 'errors' object keyed by resource type whose value is a list of reasons, and its message is built from the request as '<resource> with id=<requested id> not found'.
Emulator: 404 with only a message, in a different wording ('Channel with ID "<id>" not found.'), and no 'errors' member.
Why: the status and error code match, but a client or tool that reads the per-field 'errors' map (the structured part of the envelope) finds nothing to read. Because the message is built from the request, its form is part of the contract; the recorded text is the example of that form.
Fix: the MediaPackage error serializer must emit the 'errors' member alongside 'message' and build the message from the requested resource type and id.
Traces: mediapackage-channel-lifecycle-tags-logs#teardown#13 (cluster also covers mediapackage-credential-rotation#teardown#7, mediapackage-not-found-error-paths#task#1, mediapackage-origin-endpoint-lifecycle#teardown#14).

Cause 4 of 8 - DeleteChannel 404 body omits the 'errors' member (fixture mediapackage-not-found-error-paths, step task#2).
AWS: Smithy: DeleteChannel DELETE /channels/{Id}, NotFoundException(404); recorded behavior only - same envelope as Cause 3, errors {'channel': ['with id=<id> not found']}.
Emulator: 404 with message only, no 'errors'.
Why: same as Cause 3 on the delete path, which is exactly where tear-down tooling inspects the error.
Fix: same error serializer / the DeleteChannel not-found path.
Traces: mediapackage-not-found-error-paths#task#2.

Cause 5 of 8 - DescribeOriginEndpoint 404 body omits the 'errors' member (fixture mediapackage-not-found-error-paths, step task#3).
AWS: recorded behavior only - errors {'originEndpoint': ['with id=<id> not found']} with the matching request-derived message; NotFoundException @httpError 404.
Emulator: 404 with message only, no 'errors'.
Why: same as Cause 3 for origin endpoints; the resource-type key is what tells a client which resource was missing.
Fix: same serializer, origin-endpoint lookup path.
Traces: mediapackage-not-found-error-paths#task#3 (cluster also covers mediapackage-origin-endpoint-lifecycle#teardown#12).

Cause 6 of 8 - DeleteOriginEndpoint 404 body omits the 'errors' member (fixture mediapackage-not-found-error-paths, step task#4).
AWS: recorded behavior only - errors {'originEndpoint': ['with id=<id> not found']}; NotFoundException @httpError 404.
Emulator: 404 with message only, no 'errors'.
Why: same as Cause 4 on the origin-endpoint delete path.
Fix: same serializer / DeleteOriginEndpoint not-found path.
Traces: mediapackage-not-found-error-paths#task#4.

Cause 7 of 8 - CreateChannel duplicate-id 422 body omits the 'errors' member (fixture mediapackage-conflict-error-paths, step task#3).
AWS: Smithy: UnprocessableEntityException @httpError 422, doc 'The parameters sent in the request are not valid'; recorded behavior only - the duplicate-create body carries errors {'channel': ['with id=<id> already exists']} and a request-derived message of that same form.
Emulator: 422 with message only ('A channel with ID "<id>" already exists.'), no 'errors'.
Why: status and code are right, but the structured conflict detail a client uses to decide whether to adopt the existing channel is missing.
Fix: same serializer; the CreateChannel conflict path must populate the per-resource errors map.
Traces: mediapackage-conflict-error-paths#task#3.

Cause 8 of 8 - DeleteChannel-with-attached-endpoints 422 body omits the 'errors' member (fixture mediapackage-conflict-error-paths, step task#4).
AWS: UnprocessableEntityException @httpError 422; recorded behavior only - errors {'channel': ['cannot be deleted because 1 OriginEndpoint is attached to it']}, message of the same form, with the attached-endpoint count taken from state.
Emulator: 422 with message only ('... still has 1 origin endpoint(s); delete them before deleting the channel.'), no 'errors'.
Why: the dependency violation is reported only as prose, so a client cannot programmatically learn that the blocker is attached origin endpoints.
Fix: same serializer; the DeleteChannel dependency check must emit the errors map.
Traces: mediapackage-conflict-error-paths#task#4.

No open issue covers these; the open MediaPackage-family issues concern mediapackagev2 and mediapackage-vod, which are different services.

## Evidence
Replay (trace set simparity-wire-aws-v1):
Status/errortype divergences:
- mediapackage-listing-and-validation#task#5 ListChannels(maxResults=10000): expected 422, got 400; $headers.x-amzn-errortype got 'ValidationException' (recorded value not asserted, but 400/ValidationException is not a modeled MediaPackage error); $.errors missing (expected {'maxResults': 'must be an integer between 1 and 50'}); message got "1 validation error detected: Value '10000' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 1000".
- mediapackage-not-found-error-paths#task#5 CreateOriginEndpoint(unknown channelId): expected 422, got 404; $headers.x-amzn-errortype expected 'UnprocessableEntityException', got 'NotFoundException'; $.errors missing (expected {'channelId': ['not found']}).
Body-envelope divergences (status matches, 'errors' absent):
- mediapackage-channel-lifecycle-tags-logs#teardown#13 DescribeChannel 404: $.errors missing (expected {'channel': ['with id=<id> not found']}); message 'Channel with ID "<id>" not found.' vs recorded '<channel> with id=<id> not found'. 4 steps affected.
- mediapackage-not-found-error-paths#task#2 DeleteChannel 404: $.errors missing ({'channel': [...]}).
- mediapackage-not-found-error-paths#task#3 DescribeOriginEndpoint 404: $.errors missing ({'originEndpoint': [...]}). 2 steps affected.
- mediapackage-not-found-error-paths#task#4 DeleteOriginEndpoint 404: $.errors missing ({'originEndpoint': [...]}).
- mediapackage-conflict-error-paths#task#3 CreateChannel 422: $.errors missing ({'channel': ['with id=<id> already exists']}).
- mediapackage-conflict-error-paths#task#4 DeleteChannel 422: $.errors missing ({'channel': ['cannot be deleted because 1 OriginEndpoint is attached to it']}).
Statuses and error codes are quoted exactly; recorded message texts are examples of the request-derived form.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 corpus against the emulator built from this checkout.
2. GET /channels?maxResults=10000 (fixture mediapackage-listing-and-validation task#5): emulator answers 400 ValidationException instead of 422 UnprocessableEntityException.
3. POST /origin_endpoints with a channelId that does not exist (mediapackage-not-found-error-paths task#5): emulator answers 404 NotFoundException instead of 422 UnprocessableEntityException.
4. GET/DELETE /channels/<missing-id> and GET/DELETE /origin_endpoints/<missing-id> (mediapackage-not-found-error-paths task#2-#4, mediapackage-channel-lifecycle-tags-logs teardown#13): 404 bodies contain only 'message', no 'errors' map.
5. CreateChannel with an existing id, and DeleteChannel while an origin endpoint is attached (mediapackage-conflict-error-paths task#3, task#4): 422 bodies contain only 'message', no 'errors' map.
