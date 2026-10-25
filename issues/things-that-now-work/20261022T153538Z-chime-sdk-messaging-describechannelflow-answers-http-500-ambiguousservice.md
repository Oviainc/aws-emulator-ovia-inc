Issue-Source: trace-sweep-divergence
Issue-Trace: chime-sdk-messaging-channel-flow-create-with-uninvokable-lambda-rejected
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chime-sdk-messaging-channel-flow-create-with-uninvokable-lambda-rejected
Issue-Shape-Id: com.amazonaws.chimesdkmessaging#ChimeMessagingService
Issue-Key: chime-sdk-messaging|DescribeChannelFlow|4xx->5xx|$.Code
Issue-Key: chime-sdk-messaging|DescribeChannelFlow|4xx->5xx|$.Message
Issue-Key: chime-sdk-messaging|DescribeChannelFlow|4xx->5xx|$headers.content-type
Issue-Key: chime-sdk-messaging|DescribeChannelFlow|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: chime-sdk-messaging|DescribeChannelFlow|4xx->5xx|status
Issue-Commit: ad11f7fbecf6638207f065b52c56ab3794bd36ad

# chime-sdk-messaging DescribeChannelFlow answers HTTP 500 AmbiguousService instead of the modeled 403 ForbiddenException

> _Note: this issue was identified against the code base at commit `ad11f7fbecf6638207f065b52c56ab3794bd36ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeChannelFlow

## Description
Cause 1 of 1: a GET on the chime-sdk-messaging DescribeChannelFlow route never reaches the operation's handler. The request is refused by the emulator's front-door service-resolution layer, which keys an inbound request on the sigv4 signing name rather than on the request's endpoint prefix / Host and the restJson1 method+URI. Chime is a service family in which five distinct AWS services deliberately share the signing name 'chime' (chime, chime-sdk-identity, chime-sdk-messaging, chime-sdk-meetings, chime-sdk-voice, chime-sdk-media-pipelines each carry their OWN endpointPrefix), so that key is not a service identity and the router refuses to serve any of the candidates, emitting an unmodeled 'AmbiguousService' error as a 500.

AWS: Smithy (services/service-com-amazonaws-chimesdkmessaging-chimemessagingservice/smithy-model/com-amazonaws-chimesdkmessaging-chimemessagingservice.json) - DescribeChannelFlow is restJson1 GET /channel-flows/{ChannelFlowArn}, success code 200, input member ChannelFlowArn (@httpLabel, required, target ChimeArn, @length min 5 max 1600, @pattern ^arn:...). Its modeled errors are BadRequestException (400), ForbiddenException (403), ServiceFailureException (500), ServiceUnavailableException (503), ThrottledClientException (429), UnauthorizedClientException (401). ForbiddenException is an @error("client") structure with httpError 403 and members Code (ErrorCode enum) and Message (String). 'AmbiguousService' is not a modeled error of this service, nor a member of the ErrorCode enum. The recorded response for this step is HTTP 403 with body {"Code": "Forbidden", "Message": "An invalid App Instance ARN was supplied for the input parameter."} and Content-Type application/json - i.e. describing a channel flow whose ARN does not belong to a resolvable/authorized app instance is rejected as ForbiddenException with Code 'Forbidden'. The recorded Message text is an EXAMPLE; the requirement is the 403 status and the Code 'Forbidden' in the restJson1 error shape. restJson1 also fixes the error Content-Type at application/json and the error code is carried in x-amzn-errortype.

Emulator: the step returns HTTP 500 with no $.Code and no $.Message at all, Content-Type 'application/x-amz-json-1.1' (an AWS JSON 1.1 framing this restJson1 service never uses) and header x-amzn-errortype: AmbiguousService. So the status class is wrong (server fault instead of a terminal client error), the error code is unmodeled, the modeled ForbiddenException body members are absent, and the response is framed in the wrong protocol.

Why: a client relying on AWS behavior cannot distinguish an authorization/ARN-scoping rejection from an emulator server fault. Because 500 is retryable, botocore and application retry loops spin to max attempts and then surface an error code AWS never returns, instead of failing fast on a 403 ForbiddenException. Error-handling branches keyed on Code == 'Forbidden' never fire, and any client parsing the body with the restJson1 reader gets nothing usable because the payload is framed as application/x-amz-json-1.1. No DescribeChannelFlow handler logic is reachable at all while this holds.

Fix: the front-door service-resolution/routing layer that maps an inbound request to an emulated service - not the DescribeChannelFlow handler, whose code never runs. Resolution must key on the request's endpoint prefix / Host header (messaging-chime for this service) and, where that is absent, on the operation's restJson1 method+URI, with the sigv4 signing name used only as a weak hint; a signing name shared across a service family must never be treated as a service identity. Secondarily, a resolution failure must be serialized through the target service's protocol error serializer (restJson1: Content-Type application/json, terminal 4xx) rather than as a retryable AWS-JSON 1.1 500. The same router defect is already filed behaviorally for this service in open issue 20261022T134053Z-chime-sdk-messaging-is-entirely-unreachable-the-router.md (and for siblings in 20261022T042904Z-chime-sdk-identity-is-entirely-unreachable-the-router.md, 20261022T075458Z-chime-sdk-media-pipelines-is-entirely-unreachable-the.md, 20261022T134244Z-chime-sdk-meetings-createmeetingwithattendees-and-deletemeeting-are-refused.md); NONE of those carries this wire fixture's replay key, so this divergence is filed separately and must be re-replayed against the fixture step below.

Traces: chime-sdk-messaging-channel-flow-create-with-uninvokable-lambda-rejected#task#5 (DescribeChannelFlow, expected 403 -> got 500).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, /__admin/health 200, data listener accepting; state_reset_failures=0, so this result is not contaminated by a prior fixture's state).

Divergence cluster (verdict: mismatch), fixture chime-sdk-messaging-channel-flow-create-with-uninvokable-lambda-rejected, step task#5, operation DescribeChannelFlow, service chime-sdk-messaging:
  status: expected 403, got 500
  $.Code: missing (expected 'Forbidden', got None)
  $.Message: missing (expected 'An invalid App Instance ARN was supplied for the input parameter.', got None)   [message text is an example; the 403 + Code 'Forbidden' are the requirement]
  $headers.content-type: value_mismatch (expected 'application/json', got 'application/x-amz-json-1.1')
  $headers.x-amzn-errortype: shape_mismatch (expected a modeled chime-sdk-messaging error type, got 'AmbiguousService')
affected_count: 1 (this cluster's only member is the representative step).

Model citation: DescribeChannelFlow = restJson1 GET /channel-flows/{ChannelFlowArn}, 200 on success; modeled errors BadRequestException 400, ForbiddenException 403, ServiceFailureException 500, ServiceUnavailableException 503, ThrottledClientException 429, UnauthorizedClientException 401; ForbiddenException {Code: ErrorCode, Message: String}, httpError 403. 'AmbiguousService' appears nowhere in the model.

Corroborating context from the same replay: the corpus's 8 chime fixtures all abort in setup#1 because chime-sdk-identity CreateAppInstance returns 500 instead of 201 (already filed as 20261022T095752Z-chime-sdk-identity-createappinstance-returns-http-500-instead-of-201-blocking.md), which is why 70 of 79 steps are blocked. The DescribeChannelFlow step above is a distinct, independently observed chime-sdk-messaging wire divergence that no open issue's replay key covers.

## Steps to Reproduce
1. Build and boot the emulator from this checkout (the /__admin/health endpoint answers 200 and the data listener accepts).
2. Replay wire fixture 'chime-sdk-messaging-channel-flow-create-with-uninvokable-lambda-rejected' from trace set simparity-wire-aws-v1 and watch step 'task#5'.
3. That step issues the chime-sdk-messaging DescribeChannelFlow request: restJson1 GET /channel-flows/{ChannelFlowArn} against endpoint prefix messaging-chime, sigv4-signed with signing name 'chime', for a channel-flow ARN that is not resolvable/authorized for the caller's app instance.
4. Expected (recorded AWS response): HTTP 403, Content-Type application/json, body {"Code": "Forbidden", "Message": "An invalid App Instance ARN was supplied for the input parameter."} (ForbiddenException; the exact message text is an example, the 403 and Code 'Forbidden' are the requirement).
5. Actual: HTTP 500, Content-Type application/x-amz-json-1.1, header x-amzn-errortype: AmbiguousService, and no Code/Message members in the body.
6. Equivalent by hand: point a botocore chime-sdk-messaging client (region us-east-1, endpoint_url=<emulator>) at the emulator and call describe_channel_flow(ChannelFlowArn="arn:aws:chime:us-east-1:<account>:app-instance/<other-instance>/channel-flow/<id>"). Observe the retryable 500 AmbiguousService refusal instead of a terminal ForbiddenException.
7. Re-replay chime-sdk-messaging-channel-flow-create-with-uninvokable-lambda-rejected#task#5 to confirm the fix.
