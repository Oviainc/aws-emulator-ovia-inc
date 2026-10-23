Issue-Source: trace-sweep-divergence
Issue-Trace: kinesis-video-stream-get-data-endpoint
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesis-video-stream-get-data-endpoint kinesisvideo-get-data-endpoint
Issue-Shape-Id: com.amazonaws.kinesisvideo#KinesisVideo_20170930
Issue-Key: kinesisvideo|GetDataEndpoint|2xx->5xx|$
Issue-Key: kinesisvideo|GetDataEndpoint|2xx->5xx|$.DataEndpoint
Issue-Key: kinesisvideo|GetDataEndpoint|2xx->5xx|$headers.content-type
Issue-Key: kinesisvideo|GetDataEndpoint|2xx->5xx|status
Issue-Key: kinesisvideo|GetDataEndpoint|4xx->5xx|$.Message
Issue-Key: kinesisvideo|GetDataEndpoint|4xx->5xx|$headers.content-type
Issue-Key: kinesisvideo|GetDataEndpoint|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: kinesisvideo|GetDataEndpoint|4xx->5xx|status
Issue-Key: kinesisvideo|GetDataEndpoint|4xx->5xx|$.message
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# kinesisvideo GetDataEndpoint returns AmbiguousService 500 so the data plane is unreachable

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDataEndpoint

## Description
This issue bundles 4 causes, all on kinesisvideo GetDataEndpoint, which share ONE emulator defect. A fixer must close all 4 (the representative in Issue-Trace/Issue-Step plus the three fixture#steps named below) before submitting.

SHARED ROOT CAUSE: the emulator's front-door router resolves an inbound restJson1 request to a registered service crate using only the SigV4 credential-scope service name / endpoint prefix. Five registered services share endpointPrefix 'kinesisvideo', so the router finds five candidates, refuses to pick one, and answers HTTP 500 with the unmodeled error code 'AmbiguousService' rather than dispatching on HTTP method + request URI. POST /getDataEndpoint belongs unambiguously to the Kinesis Video control plane, so the collision is resolvable by route.

Smithy (services/service-com-amazonaws-kinesisvideo-kinesisvideo-20170930): endpointPrefix/sigv4Name 'kinesisvideo', protocol restJson1; GetDataEndpoint POST /getDataEndpoint, @http code 200, output member DataEndpoint; input APIName required (enum), StreamName @length 1..256 @pattern ^[a-zA-Z0-9_.-]+$, StreamARN @pattern ^arn:[a-z\d-]+:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$; modeled errors ClientLimitExceededException 400, InvalidArgumentException 400, NotAuthorizedException 401, ResourceNotFoundException 404. 'AmbiguousService' is defined in no kinesisvideo shape.

Same-origin wire defect on every response: the error envelope is awsJson1.1 ({"__type": ...}, Content-Type 'application/x-amz-json-1.1') while kinesisvideo is restJson1, where AWS returns Content-Type 'application/json' with x-amzn-errortype naming a modeled error. Recorded messages are EXAMPLES; status, error code and content type are the requirements, except where the message is derived from the request against a modeled constraint -- then that rule is the requirement.

Why (all 4 causes): GetDataEndpoint is the only way a client discovers the data-plane endpoint for PUT_MEDIA/GET_MEDIA/GET_HLS_STREAMING_SESSION_URL and friends. With it answering 500 and an error code absent from every AWS model, no media can be ingested or read even if the data-plane services themselves worked, SDK exception matching fails, the call is retried as a server fault, and the fixture's until_status [200] poll never terminates.

Fix (all 4 causes): the front-door service-resolution/routing layer -- after the endpoint-prefix match, disambiguate same-prefix restJson1 candidates by HTTP method + URI against each candidate's @http bindings and dispatch to the single route owner, rejecting only when no candidate route matches; the generic error serializer must also emit restJson1 errors for this service. GetDataEndpoint's handler and its APIName validation are never reached today.

Cause 1 of 4 -- GetDataEndpoint success for a valid stream/APIName. AWS: 200 with DataEndpoint. Emulator: 500 AmbiguousService, DataEndpoint missing, until_status [200] unsatisfiable. Traces: kinesis-video-stream-get-data-endpoint#task#2 (also kinesisvideo-get-data-endpoint#task#2).

Cause 2 of 4 -- GetDataEndpoint success for the other addressing form / further APINames. AWS: 200 with DataEndpoint. Emulator: 500 AmbiguousService, DataEndpoint missing. Traces: kinesis-video-stream-get-data-endpoint#task#3 (cluster also covers that fixture's task#4 and kinesisvideo-get-data-endpoint#task#4/#task#5). The endpoint host/port value itself is environment-specific and is not asserted; only the presence of DataEndpoint and the 200 are.

Cause 3 of 4 -- GetDataEndpoint for a stream that does not exist. AWS: 404 ResourceNotFoundException (Smithy: ResourceNotFoundException @httpError 404; recorded Message example: 'The requested stream is not found or not active.'). Emulator: 500 AmbiguousService, Message absent. Traces: kinesis-video-stream-get-data-endpoint#task#6 (also kinesisvideo-get-data-endpoint#task#7).

Cause 4 of 4 -- GetDataEndpoint with an APIName outside the modeled enum. AWS: 400 InvalidArgumentException; the rule is the modeled APIName enum, so the message is built from the request (recorded example: "1 validation error detected: Value at 'aPIName' failed to satisfy constraint: Member must satisfy enum value set: [GET_IMAGES, LIST_FRAGMENTS, GET_DASH_STREAMING_SESSION_URL, GET_HLS_STREAMING_SESSION_URL, GET_MEDIA_FOR_FRAGMENT_LIST, PUT_MEDIA, GET_CLIP, GET_MEDIA]"). Emulator: 500 AmbiguousService with the router's message in $.message. Traces: kinesis-video-stream-get-data-endpoint#task#7 (cluster also covers that fixture's task#8 and kinesisvideo-get-data-endpoint#task#8).

Related open issue (does NOT cover these steps): 20261022T145739Z-kinesis-video-media-router-rejects-every-kinesisvideo-prefix-request-with-ambiguousservice.md reports the same router fault from the behavioral corpus and carries no wire fixture/step linkage, so these clusters are unowned.

## Evidence
Replay summary (simparity-wire-aws-v1): emulator_up=true, bootstrap_signal=false, state_reset_failures=0, steps_total=147, steps_passed=0, steps_mismatched=87, steps_blocked=60, score_pct=0.0; all 43 divergences on 'kinesisvideo'.

Expected (AWS / recorded): GetDataEndpoint(StreamName=..., APIName='GET_MEDIA') -> 200 {"DataEndpoint": "<endpoint>"} with Content-Type 'application/json'; unknown stream -> 404 ResourceNotFoundException; APIName outside the modeled enum -> 400 InvalidArgumentException naming the enum constraint.

Actual (emulator): HTTP 500 on all four, body {"__type": "AmbiguousService", "message": "5 emulated services all claim requests for 'kinesisvideo': kinesisvideo, kinesisvideo, kinesisvideo, kinesisvideo, kinesisvideo; the request is rejected rather than served by an arbitrary one"} (example message), x-amzn-errortype: AmbiguousService, Content-Type 'application/x-amz-json-1.1'. Cluster details: 'status: expected 200, got 500' with '$.DataEndpoint: missing', 'status: expected 404, got 500', 'status: expected 400, got 500' where the recorded AWS validation message is replaced by the router's AmbiguousService message, plus '$headers.content-type: value_mismatch (expected application/json, got application/x-amz-json-1.1)' and 'poll ended: 500 AmbiguousService cannot satisfy until_status [200] after 1 attempt(s)'.

## Steps to Reproduce
1. Boot the emulator built from the checkout.
2. Replay fixture kinesis-video-stream-get-data-endpoint, step task#2 -- or call botocore kinesisvideo.get_data_endpoint(StreamName=<a created stream>, APIName='GET_MEDIA') against the emulator.
3. Observe HTTP 500 Error.Code 'AmbiguousService' with Content-Type application/x-amz-json-1.1 instead of 200 + DataEndpoint with Content-Type application/json.
4. Repeat for the other bundled causes: kinesis-video-stream-get-data-endpoint#task#3 (200 + DataEndpoint), #task#6 (404 ResourceNotFoundException), #task#7 (400 InvalidArgumentException naming the APIName enum). All four must return their modeled status and error code before closing.
5. Re-run the corpus and confirm the get-data-endpoint fixtures pass.
