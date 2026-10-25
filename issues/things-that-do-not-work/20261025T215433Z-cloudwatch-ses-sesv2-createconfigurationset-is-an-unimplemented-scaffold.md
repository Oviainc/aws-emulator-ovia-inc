Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ses.cloudwatch-destination-registers-send-metric-with-resolved-dimensions cloudwatch__ses.cloudwatch-destination-never-resolves-a-dimension-it-did-not-declare
Issue-Key: cloudwatch__ses|CreateConfigurationSet|NotImplemented
Issue-Commit: 1a78f0cc1d943875c45c11a36243ccaf94b7f40f

# cloudwatch__ses: SESv2 CreateConfigurationSet is an unimplemented scaffold stub (501 NotImplemented), so no SES CloudWatch event destination can exist and neither AWS/SES dimension-resolution check can run

> _Note: this issue was identified against the code base at commit `1a78f0cc1d943875c45c11a36243ccaf94b7f40f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfigurationSet

## Description
=== Cause 1 of 1: sesv2 CreateConfigurationSet answers 501 NotImplemented, aborting both SES-to-CloudWatch metric-destination checks at setup ===
Failing check ids: cloudwatch__ses.cloudwatch-destination-registers-send-metric-with-resolved-dimensions, cloudwatch__ses.cloudwatch-destination-never-resolves-a-dimension-it-did-not-declare.
AWS: Smithy (services/service-com-amazonaws-sesv2-simpleemailservice-v2/smithy-model) models CreateConfigurationSet with ConfigurationSetName required (ConfigurationSetName, documented "up to 64 alphanumeric characters, including letters, numbers, hyphens (-) and underscores (_) only") plus optional ArchivingOptions, DeliveryOptions, MessageSecurityOptions, ReputationOptions, SendingOptions, SuppressionOptions, Tags, TrackingOptions, VdmOptions; the documented errors are AlreadyExistsException (400), BadRequestException (400), ConcurrentModificationException (500), LimitExceededException (400), NotFoundException (404) and TooManyRequestsException (429). A valid create is a 200 with an empty body, after which CreateConfigurationSetEventDestination can attach a CloudWatchDestination whose DimensionConfigurations decide which dimensions AWS/SES datapoints carry.
Emulator: the operation is still a generated scaffold. services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/create_configuration_set.rs carries the `//! SCAFFOLDED, NOT IMPLEMENTED` marker and its handle body is `Err(crate::scaffolded::Scaffolded::not_implemented("CreateConfigurationSet"))`, so every call answers HTTP 501 with code NotImplemented and message "operation CreateConfigurationSet is scaffolded but not implemented in this emulator" — a status, code and message real AWS never returns for a well-formed request. Both checks die on their first SESv2 call and never reach SendEmail or CloudWatch.
Why: the configuration set is the only container for an SES event destination, so with this 501 nothing can register a CloudWatch destination, no AWS/SES metric can be dimensioned by ses:configuration-set or a custom message tag, and the over-permissiveness guard (a dimension that was NOT declared must never be resolved) is equally unobservable. Any client mirroring the documented SES observability setup fails at step one with a 501 that looks like a server fault rather than an unimplemented feature.
Fix: implement the handler in services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/create_configuration_set.rs (store the configuration set in the sesv2 service state, refuse a duplicate name with AlreadyExistsException and an invalid name with BadRequestException) and delete the SCAFFOLDED marker line. Related and open, but for a different symptom and service token: 20261023T183615Z-sesv2-sns-the-ses-event-destination-sns-topic.md records this operation being refused 'AmbiguousService' by the sigv4-name router — that routing refusal is gone (the call now reaches the handler and gets 501), so this is the next, distinct blocker and is not covered by it. The replay report did not mark either check id already-filed or owned.
Traces: cloudwatch__ses.cloudwatch-destination-registers-send-metric-with-resolved-dimensions, cloudwatch__ses.cloudwatch-destination-never-resolves-a-dimension-it-did-not-declare.

## Evidence
Replayed behavioral suite simparity-behavioral-ga-prod-v1 against the built emulator: cloudwatch__ses 0/2 (0%), 2 failed, neither marked "ALREADY FILED" nor "OWNED by".

Both checks fail identically, at the first SESv2 call after the sending domain is established:
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateConfigurationSet operation: operation CreateConfigurationSet is scaffolded but not implemented in this emulator
raised from `sesv2.create_configuration_set(ConfigurationSetName=<unique>)` (check bodies: 'sim2real-behav-cwdim-' and 'sim2real-behav-cwabs-' configuration sets).

Expected: HTTP 200 with an empty body, after which CreateConfigurationSetEventDestination with a CloudWatchDestination (MatchingEventTypes=[SEND], DimensionConfigurations naming only a custom message tag) is accepted, a SendEmail through that configuration set publishes an AWS/SES Send datapoint, and the dimensions on that datapoint are exactly the declared ones — ses:configuration-set appears only when the destination itself declares it.
Actual: HTTP 501 NotImplemented for CreateConfigurationSet. Source of truth in the checkout: services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/create_configuration_set.rs line 4 (`//! SCAFFOLDED, NOT IMPLEMENTED`) and line 187 (`Err(crate::scaffolded::Scaffolded::not_implemented("CreateConfigurationSet"))`).

## Steps to Reproduce
1. Establish a verified SES sending domain (route53:CreateHostedZone, sesv2:CreateEmailIdentity, publish the returned Easy DKIM CNAMEs, wait for verification).
2. sesv2:CreateConfigurationSet ConfigurationSetName=sim2real-behav-cwdim-<unique>.
   Expected: HTTP 200, empty body. Observed: HTTP 501, code NotImplemented, message "operation CreateConfigurationSet is scaffolded but not implemented in this emulator".
3. (Blocked by step 2) sesv2:CreateConfigurationSetEventDestination with EventDestination={Enabled:true, MatchingEventTypes:[SEND], CloudWatchDestination:{DimensionConfigurations:[{DimensionName:<custom tag>, DimensionValueSource:MESSAGE_TAG, DefaultDimensionValue:<v>}]}}.
4. (Blocked) sesv2:SendEmail through that configuration set with the matching message tag.
5. (Blocked) cloudwatch:ListMetrics / GetMetricStatistics in namespace AWS/SES for MetricName=Send: the datapoint must carry exactly the declared dimension, and must NOT carry ses:configuration-set when the destination did not declare it (held for an absence window calibrated from the positive leg's own publication latency).
6. Re-replay both check ids in trace_ids to confirm the fix.
