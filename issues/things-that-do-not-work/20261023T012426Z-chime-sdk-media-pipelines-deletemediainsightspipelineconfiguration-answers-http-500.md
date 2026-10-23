Issue-Source: trace-sweep-divergence
Issue-Trace: chime-sdk-media-pipelines-delete-media-insights-pipeline-configuration-not-found
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chime-sdk-media-pipelines-delete-media-insights-pipeline-configuration-not-found
Issue-Shape-Id: com.amazonaws.chimesdkmediapipelines#ChimeSDKMediaPipelinesService
Issue-Key: chime-sdk-media-pipelines|DeleteMediaInsightsPipelineConfiguration|4xx->5xx|$.Code
Issue-Key: chime-sdk-media-pipelines|DeleteMediaInsightsPipelineConfiguration|4xx->5xx|$.Message
Issue-Key: chime-sdk-media-pipelines|DeleteMediaInsightsPipelineConfiguration|4xx->5xx|$.RequestId
Issue-Key: chime-sdk-media-pipelines|DeleteMediaInsightsPipelineConfiguration|4xx->5xx|$headers.content-type
Issue-Key: chime-sdk-media-pipelines|DeleteMediaInsightsPipelineConfiguration|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: chime-sdk-media-pipelines|DeleteMediaInsightsPipelineConfiguration|4xx->5xx|status
Issue-Commit: 907268654a31a237eee5f4047dd935826414301b

# chime-sdk-media-pipelines DeleteMediaInsightsPipelineConfiguration answers HTTP 500 AmbiguousService with awsJson error framing instead of the modeled 404 NotFound

> _Note: this issue was identified against the code base at commit `907268654a31a237eee5f4047dd935826414301b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteMediaInsightsPipelineConfiguration

## Description
Cause 1 of 1 — DeleteMediaInsightsPipelineConfiguration on an unknown identifier is refused at the front door with an unmodeled server error instead of the service's modeled not-found client error, and the refusal is even framed in the wrong protocol's error encoding.

AWS: Smithy (services/service-com-amazonaws-chimesdkmediapipelines-chimesdkmediapipelinesservice/smithy-model/com-amazonaws-chimesdkmediapipelines-chimesdkmediapipelinesservice.json) — ChimeSDKMediaPipelinesService, protocol restJson1. DeleteMediaInsightsPipelineConfiguration is DELETE /media-insights-pipeline-configurations/{Identifier} with success code 204 and an empty output; Identifier is an @httpLabel, required, NonEmptyString (@length max 1024, @pattern \\S). Its modeled errors are BadRequestException (400), ConflictException (409), ForbiddenException (403), NotFoundException (404), ThrottledClientException (429), UnauthorizedClientException (401), ServiceFailureException (500), ServiceUnavailableException (503). An identifier that does not resolve to an existing media-insights pipeline configuration is the NotFoundException (httpError 404) path. 'AmbiguousService' is not a modeled error of this service. The recorded response for this step confirms the wire form: HTTP 404, Content-Type application/json (restJson1 error framing), body carrying Code 'NotFound' and a Message, an x-amzn-errortype header and a RequestId (both shaped, not value-asserted). The recorded Message 'Unable to find the requested identifier' is an EXAMPLE of a not-found message, not a requirement; the 404 status and the not-found error code/errortype are the requirements.

Emulator: the step returns HTTP 500 with an x-amzn-errortype of 'AmbiguousService' and Content-Type application/x-amz-json-1.1. The response body contains none of the expected members — $.Code, $.Message and $.RequestId are all absent. So (a) the status class is wrong (server fault instead of 404 client error), (b) the error code is one AWS never returns for this service, and (c) the error is serialized with awsJson1.1 framing even though this service speaks restJson1, so a restJson1 client cannot even parse the payload it does get. The 'AmbiguousService' errortype indicates the request never reached a chime-sdk-media-pipelines handler: the front-door resolver treated the request as claimed by more than one emulated service (the Chime family all share sigv4 signing name 'chime') and refused it before dispatch, which is also why the error framing is the generic/awsJson one rather than the service's own protocol.

Why: a client relying on AWS's behavior cannot distinguish 'this configuration does not exist' from 'the service is broken'. A 404 NotFoundException is terminal and is the normal, expected outcome of a delete-if-exists / idempotent-cleanup path; a 500 is classified by the AWS SDKs as a retryable server fault, so SDK retry loops spin to their max attempts and then surface an error code (AmbiguousService) that no client's error handling has a branch for. Cleanup and teardown code that swallows NotFoundException instead aborts, and the mismatched Content-Type means the error body does not deserialize into the modeled exception at all.

Fix: the front-door service-resolution/routing layer that maps an inbound request to an emulated service — it must disambiguate this request to chime-sdk-media-pipelines by endpoint prefix / Host (media-pipelines-chime) and the restJson1 method+URI path (DELETE /media-insights-pipeline-configurations/{Identifier}) rather than by the shared sigv4 signing name 'chime', so the operation's handler and its state lookup run. Secondarily, the error serializer for this service must emit restJson1 error framing (Content-Type application/json, body Code/Message, x-amzn-errortype header, RequestId) and any resolution failure must be rendered as a terminal client error rather than a 500. Note: open behavioral issue 20261022T075458Z-chime-sdk-media-pipelines-is-entirely-unreachable-the.md describes the same router cause but is filed against the behavioral corpus and carries NO wire replay key (Issue-Corpus: simparity-behavioral-aws-v1; its Issue-Trace-Ids are check ids), so it does not cover or re-replay this wire fixture step; this issue is filed to carry it.

Traces: fixture chime-sdk-media-pipelines-delete-media-insights-pipeline-configuration-not-found, step task#1 (the only member of this cluster).

## Evidence
Fixture: chime-sdk-media-pipelines-delete-media-insights-pipeline-configuration-not-found, step task#1 (verdict: mismatch; affected_count 1).

Expected (recorded AWS response, and the Smithy-modeled error for this path):
  HTTP 404 (NotFoundException, httpError 404)
  headers.content-type: application/json  (restJson1 error framing)
  headers.x-amzn-errortype: present (shaped, not value-asserted)
  body $.Code: 'NotFound'
  body $.Message: 'Unable to find the requested identifier'  (example text; the rule is a not-found message for the supplied Identifier)
  body $.RequestId: present (shaped, not value-asserted)

Actual (emulator):
  HTTP 500
  headers.content-type: application/x-amz-json-1.1   <- wrong protocol's error framing for a restJson1 service
  headers.x-amzn-errortype: AmbiguousService          <- not a modeled error of ChimeSDKMediaPipelinesService
  body $.Code: missing
  body $.Message: missing
  body $.RequestId: missing

Replay detail, verbatim:
  status: expected 404, got 500
  $.Code: missing (expected 'NotFound', got None)
  $.Message: missing (expected 'Unable to find the requested identifier', got None)
  $.RequestId: missing (expected '<shaped-not-asserted>', got None)
  $headers.content-type: value_mismatch (expected 'application/json', got 'application/x-amz-json-1.1')
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'AmbiguousService')

Corpus summary: trace_set simparity-wire-aws-v1, 1/1 steps replayed, 0 passed, 1 mismatched (score 0.0%); emulator_up true (/__admin/health 200), state_reset_failures 0, bootstrap_signal false — so the emulator is serving and this is a fidelity divergence, not a bootstrap gap.

## Steps to Reproduce
1. Build and boot the emulator from this checkout (/__admin/health answers 200 and the data listener accepts).
2. Replay the wire corpus simparity-wire-aws-v1, or reproduce the single step directly:
3. Point a chime-sdk-media-pipelines client (sigv4 signing name 'chime', endpoint prefix 'media-pipelines-chime') at the emulator, or issue the raw request: DELETE /media-insights-pipeline-configurations/<identifier-that-does-not-exist> with a sigv4 Authorization header whose credential scope service is 'chime', Accept/Content-Type application/json.
4. Expected: HTTP 404, Content-Type application/json, body with Code 'NotFound' (NotFoundException) and a Message naming the unresolvable identifier, plus RequestId and an x-amzn-errortype header.
5. Observed: HTTP 500, Content-Type application/x-amz-json-1.1, x-amzn-errortype: AmbiguousService, and a body with no Code/Message/RequestId — the request is refused by the front-door resolver before any chime-sdk-media-pipelines handler runs.
6. Confirm the fix by re-replaying fixture chime-sdk-media-pipelines-delete-media-insights-pipeline-configuration-not-found step task#1.
