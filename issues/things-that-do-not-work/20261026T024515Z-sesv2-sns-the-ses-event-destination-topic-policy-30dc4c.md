Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2__sns.event-destination-requires-a-topic-policy-that-actually-permits-ses-to-publish
Issue-Key: sesv2__sns|CreateConfigurationSetEventDestination|NotImplemented
Issue-Commit: 13cd51356c86db6d039912819137db7f55e61de7

# sesv2__sns: the SES event-destination topic-policy check now aborts one call later — CreateConfigurationSetEventDestination is an unimplemented scaffold stub answering 501 NotImplemented

> _Note: this issue was identified against the code base at commit `13cd51356c86db6d039912819137db7f55e61de7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfigurationSetEventDestination

## Description
Attribution note: this check id's leading token is the composite 'sesv2__sns'. The per-service bootstrap gate cannot match that composite literally, but both constituent services are stood up in this checkout (sesv2 -> service-com-amazonaws-sesv2-simpleemailservice-v2, sns -> service-com-amazonaws-sns-amazonsimplenotificationservice are both in the bootstrapped set; neither appears in pending), so the verdict is FILE. The refused call is the SESv2 one.

One root cause (Cause 1 of 1).

Related OPEN issues that do NOT cover this failure: 20261025T220900Z-sesv2-sns-the-ses-event-destination-topic-policy.md reports this same check blocked on CreateConfigurationSet with replay key 'sesv2__sns|CreateConfigurationSet|NotImplemented', and 20261023T183615Z-sesv2-sns-the-ses-event-destination-sns-topic.md reports it blocked pre-dispatch with the router's non-AWS 'AmbiguousService'. Neither is what happens now: the configuration set is created successfully and the check progresses to the NEXT call, which is the one reported here. Those issues' replay keys are operation- and error-code-qualified, so they do not carry this check's current failure and the suite did not mark it owned.

Cause 1 of 1: CreateConfigurationSetEventDestination answers HTTP 501 with error code 'NotImplemented' instead of attaching an event destination to the configuration set.
AWS: Smithy (services/service-com-amazonaws-sesv2-simpleemailservice-v2/smithy-model/com-amazonaws-sesv2-simpleemailservice-v2.json): SimpleEmailService_v2 2019-09-27, restJson1. CreateConfigurationSetEventDestination is POST /v2/email/configuration-sets/{ConfigurationSetName}/event-destinations, success code 200 with an EMPTY response body (CreateConfigurationSetEventDestinationResponse declares no members). Required input: ConfigurationSetName (httpLabel), EventDestinationName, EventDestination (EventDestinationDefinition, which carries SnsDestination.TopicArn). Modeled errors — the ONLY refusals AWS gives for this operation — are AlreadyExistsException 400, BadRequestException 400, LimitExceededException 400, NotFoundException 404, TooManyRequestsException 429. No 'NotImplemented' code and no 501 status exist anywhere in this model. Whether SES validates at create time that it may publish to the named SNS topic (and which of those modeled errors it picks for a topic whose policy explicitly denies ses.amazonaws.com:Publish) is recorded behavior only — the check is marked DRAFT/unblessed on that point — so the requirement asserted here is just that the call reaches a real handler and returns 200 for a topic SES may publish to, and a modeled 4xx naming the topic/destination otherwise. Read-back is via GetConfigurationSetEventDestinations (GET on the same URI, 200, EventDestinations list; errors BadRequestException 400 / NotFoundException 404 / TooManyRequestsException 429).
Emulator: the call is refused. HTTP 501, error code 'NotImplemented', message "operation CreateConfigurationSetEventDestination is scaffolded but not implemented in this emulator". In that message everything is fixed text — "operation <Op> is scaffolded but not implemented in this emulator" — and the only variable part is the operation name taken from the request's resolved operation; nothing comes from the request body or emulator state. No event destination is stored, so the subsequent GetConfigurationSetEventDestinations read the check performs can never observe one.
Why: a client can create a configuration set but can never attach an event destination to it, so the entire SES event-publication surface (SNS/EventBridge/Firehose destinations) is unreachable and SES's authorization check against the destination topic's resource policy can never be exercised. Worse, the status/code pair is in no AWS model: an SDK cannot classify the failure as throttling or as a modeled BadRequestException, so idempotent setup code that branches on AlreadyExistsException, and retry logic that branches on TooManyRequestsException, both take the wrong path.
Fix: implement the operation in its handler, services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/create_configuration_set_event_destination.rs (the only file for this operation), with event-destination storage hung off the configuration-set record in the service's state module so GetConfigurationSetEventDestinations can read it back; delete the '//! SCAFFOLDED, NOT IMPLEMENTED' marker at the top of that file; and refuse bad requests with the modeled errors (NotFoundException for an unknown configuration set, AlreadyExistsException for a duplicate destination name, BadRequestException for an unusable destination) rather than a 501.
Traces: sesv2__sns.event-destination-requires-a-topic-policy-that-actually-permits-ses-to-publish (fails on its first CreateConfigurationSetEventDestination call, after the configuration set and both SNS topics were created successfully).

## Evidence
Suite simparity-behavioral-ga-prod-v1: sesv2__sns 0/1 passed, 1 failed. The suite marked the check neither 'ALREADY FILED' nor 'OWNED by', i.e. no open issue's replay key carries it.

Expected (AWS, restJson1): POST /v2/email/configuration-sets/<set>/event-destinations with {"EventDestinationName": "<name>", "EventDestination": {"Enabled": true, "MatchingEventTypes": [...], "SnsDestination": {"TopicArn": "<arn>"}}} returns HTTP 200 with an empty JSON body for a topic SES may publish to, and the destination is then visible through GetConfigurationSetEventDestinations; for a topic whose policy denies ses.amazonaws.com:Publish AWS answers one of the modeled 4xx errors naming the topic ARN, and leaves no destination behind.

Actual, verbatim from the replay:
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateConfigurationSetEventDestination operation: operation CreateConfigurationSetEventDestination is scaffolded but not implemented in this emulator
HTTP status 501. Neither the 501 status nor the code 'NotImplemented' appears in the sesv2 model, whose refusals for this operation are AlreadyExistsException 400 / BadRequestException 400 / LimitExceededException 400 / NotFoundException 404 / TooManyRequestsException 429. Message decomposition: fixed text "operation " + <operation name> + " is scaffolded but not implemented in this emulator"; the operation name is the only variable part.

Checkout corroboration: services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/create_configuration_set_event_destination.rs:4 still carries '//! SCAFFOLDED, NOT IMPLEMENTED' and line 165 returns Err(crate::scaffolded::Scaffolded::not_implemented("CreateConfigurationSetEventDestination")).

Progress note (this is a strictly later failure point than the filed ones): the check's earlier recorded blockers were the pre-dispatch 'AmbiguousService' router refusal and then CreateConfigurationSet answering 501. Both are gone in this run — create_configuration_set now succeeds, as do the SNS CreateTopic/SetTopicAttributes calls — and the first refusal is now on the event-destination call.

## Steps to Reproduce
1. Boot the emulator and point a botocore SESv2 client and an SNS client at it: sesv2 = boto3.client('sesv2', endpoint_url=<emulator>, region_name=<region>); sns = boto3.client('sns', endpoint_url=<emulator>, region_name=<region>).
2. sesv2.create_configuration_set(ConfigurationSetName='sim2real-cs') — this now returns 200.
3. topic = sns.create_topic(Name='sim2real-allow')['TopicArn']; sns.set_topic_attributes(TopicArn=topic, AttributeName='Policy', AttributeValue=<policy allowing ses.amazonaws.com:Publish on that topic>), then confirm it with sns.get_topic_attributes(TopicArn=topic).
4. sesv2.create_configuration_set_event_destination(ConfigurationSetName='sim2real-cs', EventDestinationName='sim2real-ed-policy-probe', EventDestination={'Enabled': True, 'MatchingEventTypes': ['SEND'], 'SnsDestination': {'TopicArn': topic}}). Expect HTTP 200 with an empty body; observe HTTP 501 ClientError (NotImplemented) 'operation CreateConfigurationSetEventDestination is scaffolded but not implemented in this emulator'.
5. Confirm nothing was stored: sesv2.get_configuration_set_event_destinations(ConfigurationSetName='sim2real-cs') returns no destination (AWS would list the one just created).
6. Full grounding: replay sesv2__sns.event-destination-requires-a-topic-policy-that-actually-permits-ses-to-publish, which creates the set, attaches an SNS event destination whose topic policy explicitly denies ses.amazonaws.com:Publish, asserts SES refuses the destination it cannot publish to, and then lifts the deny and asserts the identical call succeeds.
