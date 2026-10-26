Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ses.cloudwatch-destination-registers-send-metric-with-resolved-dimensions cloudwatch__ses.cloudwatch-destination-never-resolves-a-dimension-it-did-not-declare
Issue-Key: cloudwatch__ses|CreateConfigurationSetEventDestination|NotImplemented
Issue-Commit: ebfe4f059696b431f00c6abb397dd11b94b7073a

# cloudwatch__ses: SESv2 CreateConfigurationSetEventDestination is an unimplemented scaffold stub answering 501 NotImplemented, so no CloudWatch event destination can be created and no AWS/SES metric is ever dimensioned

> _Note: this issue was identified against the code base at commit `ebfe4f059696b431f00c6abb397dd11b94b7073a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfigurationSetEventDestination

## Description
Cause 1 of 1: the SESv2 operation that registers a CloudWatchDestination on a configuration set is a scaffold stub, so the entire SES->CloudWatch metrics-delivery surface is unreachable.
  AWS: Smithy (services/service-com-amazonaws-sesv2-simpleemailservice-v2 model): CreateConfigurationSetEventDestination, POST /v2/email/configuration-sets/{ConfigurationSetName}/event-destinations, success status 200 with CreateConfigurationSetEventDestinationResponse; required input ConfigurationSetName (@httpLabel), EventDestinationName, EventDestination (EventDestinationDefinition); modelled errors AlreadyExistsException 400, BadRequestException 400, LimitExceededException 400, NotFoundException 404, TooManyRequestsException 429. Documented as "Create an event destination. Events include message sends, deliveries, opens, clicks, bounces, and complaints." A well-formed call against an existing configuration set with EventDestination={Enabled: true, MatchingEventTypes:["SEND"], CloudWatchDestination:{DimensionConfigurations:[...]}} is accepted with HTTP 200; AWS then publishes AWS/SES metrics for sends through that configuration set, resolving into dimensions ONLY the names the destination's own DimensionConfigurations declares (DimensionValueSource=MESSAGE_TAG), and never defaulting in `ses:configuration-set` when the destination did not declare it. 501 NotImplemented is not a status this operation can return.
  Emulator: every call is refused. HTTP 501, error code `NotImplemented`, message `operation CreateConfigurationSetEventDestination is scaffolded but not implemented in this emulator`. The handler is a generated stub whose handle body is `Err(crate::scaffolded::Scaffolded::not_implemented("CreateConfigurationSetEventDestination"))` and which still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker.
  Why: a client cannot attach any event destination to a configuration set, so no AWS/SES metric is ever published for its sends and nothing downstream (dimension resolution, alarms on Send/Bounce, the negative guarantee that an undeclared dimension is never added) can be exercised. Both checks abort at setup, before a single email is sent, so neither the positive nor the over-permissiveness half of SES->CloudWatch dimension resolution is covered at all.
  Fix: implement the handler at services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/create_configuration_set_event_destination.rs (replace the not_implemented refusal, persist the event destination on the named configuration set in the service state, refuse an unknown configuration set with NotFoundException 404 and a duplicate EventDestinationName with AlreadyExistsException 400) and have the send path publish AWS/SES datapoints carrying exactly the declared DimensionConfigurations. Related but not covering: the open issue 20261026T021948Z-firehose-s3-sesv2-both-sesv2-firehose-event-destination-e5ab30.md names this same operation for the Firehose event-destination checks; it does not carry either of these two checks' replay keys, so this item is what gates the CloudWatch destination half.
  Traces: cloudwatch__ses.cloudwatch-destination-registers-send-metric-with-resolved-dimensions, cloudwatch__ses.cloudwatch-destination-never-resolves-a-dimension-it-did-not-declare.

## Evidence
Expected (AWS): sesv2.create_configuration_set_event_destination(ConfigurationSetName=<existing set>, EventDestinationName=<name>, EventDestination={"Enabled": True, "MatchingEventTypes": ["SEND"], "CloudWatchDestination": {"DimensionConfigurations": [ {DimensionName, DimensionValueSource: "MESSAGE_TAG", DefaultDimensionValue} ... ]}}) -> HTTP 200 (CreateConfigurationSetEventDestinationResponse). Subsequent sends through that configuration set produce AWS/SES `Send` datapoints dimensioned by exactly the declared dimension names — and by nothing else: `ses:configuration-set` appears only if the destination declared it.

Actual (emulator): HTTP 501 — botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateConfigurationSetEventDestination operation: operation CreateConfigurationSetEventDestination is scaffolded but not implemented in this emulator. Identical refusal in both checks, raised on the call immediately after sesv2.create_configuration_set succeeded, so neither check ever sends mail or reads AWS/SES. Source confirmation: services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/create_configuration_set_event_destination.rs lines 3-6 (SCAFFOLDED marker) and line 165 (`Err(crate::scaffolded::Scaffolded::not_implemented("CreateConfigurationSetEventDestination"))`).

## Steps to Reproduce
1. Resolve a sending domain verified for sending in SESv2 (the checks use the account's tagged DNS-baseline hosted zone, else a stand-in domain they verify themselves via Route 53 DKIM CNAMEs).
2. sesv2.create_configuration_set(ConfigurationSetName=CS) -> succeeds today.
3. sesv2.create_configuration_set_event_destination(ConfigurationSetName=CS, EventDestinationName="sim2real-cw-abs", EventDestination={"Enabled": True, "MatchingEventTypes": ["SEND"], "CloudWatchDestination": {"DimensionConfigurations": [{"DimensionName": "campaign", "DimensionValueSource": "MESSAGE_TAG", "DefaultDimensionValue": "none"}]}}).
4. Observe HTTP 501 NotImplemented instead of HTTP 200.
5. (After the fix) send through CS with that message tag and read AWS/SES `Send` via cloudwatch.get_metric_statistics: the datapoint must carry the declared dimension(s) alone, with no `ses:configuration-set` dimension defaulted in.
Replay: cloudwatch__ses.cloudwatch-destination-registers-send-metric-with-resolved-dimensions and cloudwatch__ses.cloudwatch-destination-never-resolves-a-dimension-it-did-not-declare.
