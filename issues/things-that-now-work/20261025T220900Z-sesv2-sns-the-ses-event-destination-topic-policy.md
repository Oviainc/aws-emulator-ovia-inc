Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2__sns.event-destination-requires-a-topic-policy-that-actually-permits-ses-to-publish
Issue-Key: sesv2__sns|CreateConfigurationSet|NotImplemented
Issue-Commit: ca265ed85df5a1b6b7b5ff6c2c813551ab1096d8

# sesv2__sns: the SES event-destination topic-policy check still cannot start — CreateConfigurationSet is an unimplemented scaffold stub answering 501 NotImplemented

> _Note: this issue was identified against the code base at commit `ca265ed85df5a1b6b7b5ff6c2c813551ab1096d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfigurationSet

## Description
Attribution note: this check id's leading token is the composite 'sesv2__sns'; both constituent services (sesv2, sns) are stood up in this checkout, so the per-service bootstrap gate is FILE. The refused call is the SESv2 one.

One root cause. Related OPEN issue: 20261023T183615Z-sesv2-sns-the-ses-event-destination-sns-topic.md reports this same check failing because CreateConfigurationSet was refused pre-dispatch with the non-AWS code 'AmbiguousService' by the sigv4-name router. That is no longer what happens — the request now reaches the SESv2 handler — and that issue's replay key is error-code-qualified ('sesv2__sns|CreateConfigurationSet|AmbiguousService'), so it does not carry this check's current failure. The blocker now is that the operation itself is a scaffold stub.

Cause 1 of 1: CreateConfigurationSet answers HTTP 501 'NotImplemented' instead of creating a configuration set.
AWS: Smithy (services/service-com-amazonaws-sesv2-simpleemailservice-v2/smithy-model/com-amazonaws-sesv2-simpleemailservice-v2.json): SimpleEmailService_v2 2019-09-27, restJson1, sigv4Name 'ses', endpointPrefix 'email'. CreateConfigurationSet is POST /v2/email/configuration-sets, success code 200, EMPTY response body (CreateConfigurationSetResponse declares no members); required input ConfigurationSetName, optional TrackingOptions/DeliveryOptions/ReputationOptions/SendingOptions/SuppressionOptions/VdmOptions/ArchivingOptions/MessageSecurityOptions/Tags. Modeled errors, and the only refusals AWS gives: AlreadyExistsException 400, BadRequestException 400, LimitExceededException 400, NotFoundException 404, ConcurrentModificationException 500, TooManyRequestsException 429. No 'NotImplemented' code and no 501 status exist in this model. The behavior the check then grades sits on CreateConfigurationSetEventDestination (POST /v2/email/configuration-sets/{ConfigurationSetName}/event-destinations, 200; modeled errors AlreadyExistsException 400, BadRequestException 400, LimitExceededException 400, NotFoundException 404, TooManyRequestsException 429) with an SnsDestination whose topic policy explicitly denies ses.amazonaws.com:Publish — recorded behavior only for which of those errors AWS picks and for whether the check happens at create time or at delivery time; the check itself is marked DRAFT/unblessed on that point, so the requirement asserted here is only that the create returns 200 and the destination call reaches a handler.
Emulator: the first call is refused. HTTP 501, error code 'NotImplemented', message "operation CreateConfigurationSet is scaffolded but not implemented in this emulator" — fixed text apart from the operation name, which is the only request-derived part. services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/create_configuration_set.rs still carries the '//! SCAFFOLDED, NOT IMPLEMENTED' marker and its handle() returns Err(crate::scaffolded::Scaffolded::not_implemented("CreateConfigurationSet")); no configuration set is stored.
Why: no client can create a configuration set, so an SNS event destination can never be attached and SES's authorization check against the destination topic's policy is unreachable. A client gets a status and code that are in no AWS model, so SDK error handling cannot classify the failure (it is neither throttling nor a modeled BadRequestException), and idempotent setup code that branches on AlreadyExistsException misbehaves.
Fix: implement the operation in its handler, services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/create_configuration_set.rs (the only file for this operation), plus configuration-set storage in the service's state module so the event-destination handler can resolve the set by name and apply the SNS topic-policy check; delete the SCAFFOLDED marker, and refuse bad requests with the modeled errors rather than a 501.
Traces: sesv2__sns.event-destination-requires-a-topic-policy-that-actually-permits-ses-to-publish (fails on its first SESv2 call).

## Evidence
Suite simparity-behavioral-ga-prod-v1: sesv2__sns 0/1 passed, 1 failed; the check is not owned by any open issue's replay key.

Expected (AWS, restJson1): POST /v2/email/configuration-sets with {"ConfigurationSetName": "<name>"} returns HTTP 200 with an empty JSON body, after which CreateConfigurationSetEventDestination can attach an SnsDestination and SES evaluates whether it may publish to that topic.
Actual, verbatim from the replay:
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateConfigurationSet operation: operation CreateConfigurationSet is scaffolded but not implemented in this emulator
HTTP status 501. Neither the 501 status nor the code 'NotImplemented' appears in the sesv2 model, whose CreateConfigurationSet refusals are AlreadyExistsException 400 / BadRequestException 400 / LimitExceededException 400 / NotFoundException 404 / ConcurrentModificationException 500 / TooManyRequestsException 429. The message's fixed text is "operation <Op> is scaffolded but not implemented in this emulator"; the only variable part is the operation name.

Checkout corroboration: services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/create_configuration_set.rs:4 still carries '//! SCAFFOLDED, NOT IMPLEMENTED' and line 187 returns Scaffolded::not_implemented("CreateConfigurationSet").

Context: the earlier refusal for this check was the pre-dispatch 'AmbiguousService' router error (open issue 20261023T183615Z-sesv2-sns-the-ses-event-destination-sns-topic.md). That no longer occurs in this run; the call now reaches the handler and is answered 501, which is the divergence reported here.

## Steps to Reproduce
1. Boot the emulator and point a botocore SESv2 client at it: sesv2 = boto3.client('sesv2', endpoint_url=<emulator>, region_name=<region>).
2. Call sesv2.create_configuration_set(ConfigurationSetName='sim2real-cs'). Expect HTTP 200 with an empty body; observe HTTP 501 ClientError (NotImplemented) 'operation CreateConfigurationSet is scaffolded but not implemented in this emulator'.
3. Confirm nothing was stored: sesv2.create_configuration_set_event_destination(ConfigurationSetName='sim2real-cs', EventDestinationName='sim2real-gate', EventDestination={'Enabled': True, 'MatchingEventTypes': ['SEND'], 'SnsDestination': {'TopicArn': <a topic whose policy denies ses.amazonaws.com:Publish>}}) — AWS would answer either 200 or a modeled 4xx naming the set/topic, never a 501.
4. Full grounding: replay sesv2__sns.event-destination-requires-a-topic-policy-that-actually-permits-ses-to-publish, which creates the configuration set, attaches an SNS event destination whose topic policy explicitly denies ses.amazonaws.com:Publish, and asserts SES refuses the destination it cannot publish to.
