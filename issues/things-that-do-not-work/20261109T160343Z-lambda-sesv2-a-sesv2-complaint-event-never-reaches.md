Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sesv2.sesv2-eventbridge-complaint-event-lambda-positive
Issue-Key: lambda__sesv2.sesv2-eventbridge-complaint-event-lambda-positive|probe#11|Timeout|return len(collected)
Issue-Commit: 5b2a9eedb8fa287cac2318fd09ae07f52de03ab6

# lambda__sesv2: a SESv2 Complaint event never reaches EventBridge, so an 'Email Complaint Received' rule target is never invoked

> _Note: this issue was identified against the code base at commit `5b2a9eedb8fa287cac2318fd09ae07f52de03ab6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendEmail

## Description
Cause 1 of 1: a send that produces a complaint, through a configuration set whose event destination is EventBridge and whose MatchingEventTypes include COMPLAINT, emits no 'Email Complaint Received' event on the default event bus, so an EventBridge rule targeting a Lambda relay is never invoked and the check exhausts its whole budget waiting. Failing check: lambda__sesv2.sesv2-eventbridge-complaint-event-lambda-positive.
AWS: SESv2 SendEmail (Smithy com.amazonaws.sesv2: POST /v2/email/outbound-emails, input Content required, ConfigurationSetName) to the mailbox simulator address complaint@simulator.amazonses.com is accepted (200, MessageId) and the simulator generates a complaint for it. With an EventBridge event destination on the configuration set (CreateConfigurationSetEventDestination with EventDestination.EventBridgeDestination and MatchingEventTypes containing COMPLAINT), SES publishes an event onto the bus whose source is 'aws.ses', whose detail-type is 'Email Complaint Received' and whose detail carries eventType 'Complaint' and mail.destination containing the recipient the send addressed; a rule matching {"source":["aws.ses"],"detail-type":["Email Complaint Received"]} with a Lambda target invokes that function with the event. Recorded behavior: the event arrives within the check's 300 s wait.
Emulator: no such event is ever delivered — the relay Lambda is never invoked, nothing lands in the relay queue, and the check is killed by the harness at 180 s with 'Check timed out after 180.0s'. Hypothesis: SESv2's send path emits no COMPLAINT event for the mailbox-simulator complaint address, or the configuration set's EventBridge destination is inert, so the COMPLAINT leg of the event pipeline never runs; the replay output shows only the timeout, not which leg is missing.
Why: any client that reacts to complaints out of EventBridge (suppression automation, reputation dashboards, compliance alerting) silently receives nothing — no error, just no event — so complaint handling built on SES event publishing cannot be developed or tested against the emulator.
Fix: the SESv2 send/eventing path — emission of the COMPLAINT event for a mailbox-simulator complaint recipient and its delivery through the configuration set's EventBridge event destination onto the default bus with source 'aws.ses' and detail-type 'Email Complaint Received'. Two open issues cover the sibling legs of the same pipeline and are marked as owning their own checks: 20261101T210635Z-lambda-sesv2-sesv2-send-bounce-events-never-reach.md (Send/Bounce) and 20261102T110414Z-lambda-sesv2-a-sesv2-delivery-event-never-reaches.md (Delivery). Neither carries this check's replay key nor names the COMPLAINT event type, so the complaint leg must be re-replayed separately.
Traces: lambda__sesv2.sesv2-eventbridge-complaint-event-lambda-positive.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator; verdict [mismatch].
Expected (AWS): within the 300 s wait, at least one relayed event with source 'aws.ses', detail-type 'Email Complaint Received', detail.eventType 'Complaint' and the recipient complaint@simulator.amazonses.com present in detail.mail.destination.
Actual (emulator): Error: Check timed out after 180.0s — no event is ever relayed, so the first assertion ("no 'Email Complaint Received' event relayed") is never even reached.

## Steps to Reproduce
1. Verify a sending identity, create an SQS relay queue, an IAM role and a relay Lambda that forwards its event to the queue.
2. sesv2.create_configuration_set(<cs>) and sesv2.create_configuration_set_event_destination(ConfigurationSetName=<cs>, EventDestination={Enabled: true, MatchingEventTypes: ['COMPLAINT'], EventBridgeDestination: {EventBusArn: <default bus>}}).
3. events.put_rule(EventPattern={"source":["aws.ses"],"detail-type":["Email Complaint Received"]}), lambda.add_permission for events.amazonaws.com with the rule ARN, events.put_targets(<relay lambda>).
4. sesv2.send_email(FromEmailAddress=<verified sender>, Destination={ToAddresses:['complaint@simulator.amazonses.com']}, Content={Simple: {...}}, ConfigurationSetName=<cs>).
5. Poll the relay queue: AWS delivers the 'Email Complaint Received' event; the emulator delivers nothing until the check's budget expires.
6. Re-replay lambda__sesv2.sesv2-eventbridge-complaint-event-lambda-positive.
