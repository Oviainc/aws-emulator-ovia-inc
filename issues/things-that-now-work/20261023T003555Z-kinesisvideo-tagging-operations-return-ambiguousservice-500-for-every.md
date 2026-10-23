Issue-Source: trace-sweep-divergence
Issue-Trace: kinesis-video-stream-tags-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesis-video-stream-tags-lifecycle kinesisvideo-stream-tags kinesis-video-stream-arn-tag-resource-surface
Issue-Shape-Id: com.amazonaws.kinesisvideo#KinesisVideo_20170930
Issue-Key: kinesisvideo|ListTagsForStream|2xx->5xx|$
Issue-Key: kinesisvideo|ListTagsForStream|2xx->5xx|$headers.content-type
Issue-Key: kinesisvideo|ListTagsForStream|2xx->5xx|status
Issue-Key: kinesisvideo|TagStream|2xx->5xx|$
Issue-Key: kinesisvideo|TagStream|2xx->5xx|$headers.content-type
Issue-Key: kinesisvideo|TagStream|2xx->5xx|status
Issue-Key: kinesisvideo|UntagStream|2xx->5xx|$
Issue-Key: kinesisvideo|UntagStream|2xx->5xx|$headers.content-type
Issue-Key: kinesisvideo|UntagStream|2xx->5xx|status
Issue-Key: kinesisvideo|TagStream|4xx->5xx|$.message
Issue-Key: kinesisvideo|TagStream|4xx->5xx|$headers.content-type
Issue-Key: kinesisvideo|TagStream|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: kinesisvideo|TagStream|4xx->5xx|status
Issue-Key: kinesisvideo|ListTagsForStream|4xx->5xx|$.Message
Issue-Key: kinesisvideo|ListTagsForStream|4xx->5xx|$headers.content-type
Issue-Key: kinesisvideo|ListTagsForStream|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: kinesisvideo|ListTagsForStream|4xx->5xx|status
Issue-Key: kinesisvideo|ListTagsForStream|4xx->5xx|$.message
Issue-Key: kinesisvideo|ListTagsForStream|2xx->5xx|$.Tags
Issue-Key: kinesisvideo|TagStream|4xx->5xx|$.Message
Issue-Key: kinesisvideo|TagResource|4xx->5xx|$.Message
Issue-Key: kinesisvideo|TagResource|4xx->5xx|$headers.content-type
Issue-Key: kinesisvideo|TagResource|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: kinesisvideo|TagResource|4xx->5xx|status
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# kinesisvideo tagging operations return AmbiguousService 500 for every TagStream UntagStream and ListTags step

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForStream

## Description
This issue bundles 10 causes, all of the kinesisvideo tagging surface, which share ONE emulator defect. A fixer must close all 10 (the representative in Issue-Trace/Issue-Step plus every fixture#step named under the causes below) before submitting.

SHARED ROOT CAUSE: the emulator's front-door router maps an inbound restJson1 request to a service crate using only the SigV4 credential-scope service name / endpoint prefix. Five registered services share endpointPrefix 'kinesisvideo', so the router finds five candidates, refuses to choose, and returns HTTP 500 with the unmodeled error code 'AmbiguousService' instead of dispatching on HTTP method + request URI. The five services' URI spaces are disjoint, so every route below names exactly one service.

Smithy (services/service-com-amazonaws-kinesisvideo-kinesisvideo-20170930): endpointPrefix/sigv4Name 'kinesisvideo', protocol restJson1; ListTagsForStream POST /listTagsForStream (code 200), TagStream POST /tagStream (200), UntagStream POST /untagStream (200), TagResource POST /TagResource (200). Tags maps/lists are @length 1..50; ResourceARN/StreamARN are @length 1..1024 @pattern ^arn:[a-z\d-]+:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$. 'AmbiguousService' is in no kinesisvideo shape; modeled errors here are ClientLimitExceededException 400, InvalidArgumentException 400, InvalidResourceFormatException 400, NotAuthorizedException 401, ResourceNotFoundException 404, TagsPerResourceExceededLimitException 400, AccessDeniedException 401.

Same-origin wire defect on every response: the error envelope is awsJson1.1 ({"__type": ...} with Content-Type 'application/x-amz-json-1.1') though kinesisvideo is restJson1, where AWS returns Content-Type 'application/json' and sets x-amzn-errortype to a modeled error name. Recorded messages below are EXAMPLES; the requirements are the status, the error code and the content type -- except where the message is built from the request against a modeled constraint, in which case that rule is the requirement.

Why (all 10 causes): tagging is completely unusable -- tags cannot be set, removed or read, and neither the empty-body 200 of a successful TagStream/UntagStream nor the modeled 400/404 rejections are observable. The 500 carries an error code that exists in no AWS model, so SDK exception matching fails and the call is retried as a server fault; fixture polls for a 200 never terminate.

Fix (all 10 causes): the front-door service-resolution/routing layer -- after the endpoint-prefix match, disambiguate among same-prefix restJson1 candidates by matching HTTP method + URI against each candidate's @http bindings and dispatch to the single route owner, rejecting only when no candidate route matches. The generic error serializer must also emit restJson1 errors for this service. The tagging handlers and their validation are never reached today.

Cause 1 of 10 -- ListTagsForStream after tagging. AWS: 200 with Tags map and NextToken null. Emulator: 500 AmbiguousService instead of the modeled body; the until_status [200] poll cannot be satisfied. Traces: kinesis-video-stream-tags-lifecycle#task#2 (cluster also covers that fixture's task#4 and task#6).

Cause 2 of 10 -- TagStream success. AWS: 200 with an empty body (Smithy: TagStreamOutput has no members). Emulator: 500 AmbiguousService. Traces: kinesis-video-stream-tags-lifecycle#task#3 (also kinesisvideo-stream-tags#task#3).

Cause 3 of 10 -- UntagStream success. AWS: 200 with an empty body. Emulator: 500 AmbiguousService. Traces: kinesis-video-stream-tags-lifecycle#task#5 (cluster also covers that fixture's task#7 and kinesisvideo-stream-tags#task#5/#task#7).

Cause 4 of 10 -- TagStream over the 50-tag limit. AWS: 400 InvalidArgumentException; the rule is the modeled Tags @length max 50, so the message is derived from the request (recorded example: "1 validation error detected: Value at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50"). Emulator: 500 AmbiguousService with the router's message. Traces: kinesis-video-stream-tags-lifecycle#task#8 (also kinesisvideo-stream-tags#task#13).

Cause 5 of 10 -- ListTagsForStream for an unknown stream. AWS: 404 ResourceNotFoundException (recorded Message example: 'The requested stream is not found or not active.'). Emulator: 500 AmbiguousService. Traces: kinesis-video-stream-tags-lifecycle#task#9.

Cause 6 of 10 -- ListTagsForStream with a malformed streamARN. AWS: 400 InvalidArgumentException naming the modeled ARN pattern (recorded example: "1 validation error detected: Value at 'streamARN' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:[a-z\\d-]+:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+"). Emulator: 500 AmbiguousService. Traces: kinesis-video-stream-tags-lifecycle#task#10.

Cause 7 of 10 -- ListTagsForStream read-back of a single tag. AWS: 200 with Tags and NextToken null. Emulator: 500 AmbiguousService, Tags missing; until_status [200] never satisfied. Traces: kinesisvideo-stream-tags#task#2.

Cause 8 of 10 -- ListTagsForStream read-back after multiple tags are added. AWS: 200 with the full Tags map. Emulator: 500 AmbiguousService. Traces: kinesisvideo-stream-tags#task#4 (cluster also covers that fixture's task#6).

Cause 9 of 10 -- TagStream against an unknown stream. AWS: 404 ResourceNotFoundException (recorded Message example: 'The requested stream is not found or not active.'). Emulator: 500 AmbiguousService. Traces: kinesisvideo-stream-tags#task#12.

Cause 10 of 10 -- TagResource with a malformed resourceARN. AWS: 400 InvalidArgumentException naming the modeled ResourceARN pattern (recorded example: "1 validation error detected: Value at 'resourceARN' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:[a-z\\d-]+:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+"). Emulator: 500 AmbiguousService, Message absent. Traces: kinesis-video-stream-arn-tag-resource-surface#task#7.

Related open issue (does NOT cover these steps): 20261022T145739Z-kinesis-video-media-router-rejects-every-kinesisvideo-prefix-request-with-ambiguousservice.md reports the same router fault from the behavioral corpus with no wire fixture/step linkage.

## Evidence
Replay summary (simparity-wire-aws-v1): emulator_up=true, bootstrap_signal=false, state_reset_failures=0, steps_total=147, steps_passed=0, steps_mismatched=87, steps_blocked=60, score_pct=0.0; all 43 divergences on 'kinesisvideo'.

Expected (AWS / recorded): ListTagsForStream -> 200 {"NextToken": null, "Tags": {...}}; TagStream -> 200 {}; UntagStream -> 200 {}; TagStream with >50 tags -> 400 InvalidArgumentException; ListTagsForStream/TagStream on an unknown stream -> 404 ResourceNotFoundException; malformed streamARN/resourceARN -> 400 InvalidArgumentException. All with Content-Type 'application/json' and x-amzn-errortype naming the modeled error on failures.

Actual (emulator): HTTP 500 for all of them, body {"__type": "AmbiguousService", "message": "5 emulated services all claim requests for 'kinesisvideo': kinesisvideo, kinesisvideo, kinesisvideo, kinesisvideo, kinesisvideo; the request is rejected rather than served by an arbitrary one"} (example message), x-amzn-errortype: AmbiguousService, Content-Type 'application/x-amz-json-1.1'. Cluster details include 'status: expected 200, got 500', 'status: expected 400, got 500', 'status: expected 404, got 500', '$headers.content-type: value_mismatch (expected application/json, got application/x-amz-json-1.1)', and for the validation cases the recorded AWS validation message is replaced wholesale by the router's AmbiguousService message.

## Steps to Reproduce
1. Boot the emulator built from the checkout.
2. Replay fixture kinesis-video-stream-tags-lifecycle, step task#2 (ListTagsForStream) -- or call botocore kinesisvideo.list_tags_for_stream(StreamName=<a created stream>) against the emulator.
3. Observe HTTP 500 Error.Code 'AmbiguousService' with Content-Type application/x-amz-json-1.1 instead of 200 + Tags with Content-Type application/json.
4. Repeat for every bundled cause: kinesis-video-stream-tags-lifecycle#task#3/#task#5/#task#8/#task#9/#task#10, kinesisvideo-stream-tags#task#2/#task#4/#task#12, kinesis-video-stream-arn-tag-resource-surface#task#7. Each must return its modeled status and error code (or empty 200) before closing.
5. Re-run the corpus to confirm the tagging fixtures pass.
