Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2__sns.event-destination-requires-a-topic-policy-that-actually-permits-ses-to-publish
Issue-Key: sesv2__sns|CreateConfigurationSet|AmbiguousService
Issue-Commit: 3dc196f76d19116b5d570e1dc438e2b93d0e0d47

# sesv2__sns: the SES event-destination/SNS topic-policy check cannot start — CreateConfigurationSet is refused 'AmbiguousService' by the sigv4-name router

> _Note: this issue was identified against the code base at commit `3dc196f76d19116b5d570e1dc438e2b93d0e0d47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfigurationSet

## Description
Attribution note: the check id's leading token is the composite 'sesv2__sns'; both constituent services (sesv2, sns) are stood up in this checkout, so the per-service bootstrap gate is FILE. The refused call is the SESv2 one.

Cause 1 of 1: every sigv4-'ses' request is refused with the non-AWS code AmbiguousService instead of being dispatched to SESv2.
AWS: Smithy (services/service-com-amazonaws-sesv2-simpleemailservice-v2): SimpleEmailService_v2, version 2019-09-27, protocols [restJson1], sigv4Name 'ses', awsService.endpointPrefix 'email'. CreateConfigurationSet is an ordinary restJson1 POST under /v2/email/... returning 200 with its modeled output; refusals are the operation's modeled client errors (BadRequestException 400, AlreadyExistsException 400, LimitExceededException 400, NotFoundException 404, TooManyRequestsException 429). Three further on-disk models share sigv4Name 'ses' (SES v1 awsQuery, AmazonPinpointEmailService restJson1, MailManagerSvc awsJson1_0/rpcv2Cbor), so the signing name is not a service identity. AmbiguousService is in no AWS model.
Emulator: the call never reaches a handler. It is answered with code 'AmbiguousService' and the message "4 emulated services all claim requests for 'ses': ses, ses, ses, ses; the request is rejected rather than served by an arbitrary one" (message quoted as an example; the code and its 5xx class are the requirement), which botocore retries to exhaustion ('reached max retries: 3').
Why: the configuration set that this scenario's SNS event destination hangs off can never be created, so the behavior under test — that an event destination only publishes when the SNS topic policy actually permits ses to publish — is unreachable. More broadly, no client can configure any part of the SESv2 surface, and the returned code is unclassifiable by the SDK so the full retry budget is burned on a non-retryable condition.
Fix: the request router / service-registry resolution layer that selects a target service before operation dispatch; the four registrations claiming sigv4 name 'ses' must resolve to exactly one target per request using protocol + endpoint prefix + request shape, never by signing name alone. This is the same router defect reported in 20261023T120410Z-ses-v1-and-v2-is-entirely-unreachable-the.md and 20261022T194351Z-firehose-s3-sesv2-ses-v2-is-unreachable-the-router-answers-ambiguousservice.md; neither carries this check's replay key.
Traces: sesv2__sns.event-destination-requires-a-topic-policy-that-actually-permits-ses-to-publish.

## Evidence
Suite simparity-behavioral-aws-v1: sesv2__sns 0/1 passed, 1 failed.

Expected: CreateConfigurationSet returns 200 with its modeled output so the SNS event destination can be attached.
Actual: botocore.exceptions.ClientError: An error occurred (AmbiguousService) when calling the CreateConfigurationSet operation (reached max retries: 3): 4 emulated services all claim requests for 'ses': ses, ses, ses, ses; the request is rejected rather than served by an arbitrary one.

## Steps to Reproduce
1. Point a SESv2 client at the emulator and call sesv2.create_configuration_set(ConfigurationSetName=<name>).
2. Expect HTTP 200 with the modeled CreateConfigurationSetResponse; the emulator answers a 5xx with error code AmbiguousService and botocore retries it 3 times.
3. Full grounding: replay sesv2__sns.event-destination-requires-a-topic-policy-that-actually-permits-ses-to-publish, which then attaches an SNS event destination and checks publication is gated on the topic policy.
