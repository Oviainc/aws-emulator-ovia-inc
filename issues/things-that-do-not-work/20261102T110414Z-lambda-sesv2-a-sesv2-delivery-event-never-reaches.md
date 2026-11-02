Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sesv2.sesv2-eventbridge-delivery-event-lambda-positive
Issue-Key: lambda__sesv2.sesv2-eventbridge-delivery-event-lambda-positive|probe#11|Timeout|return len(collected)
Issue-Commit: fbef683f21970efb4631f2658975dd3b2c5af139

# lambda__sesv2: a SESv2 DELIVERY event never reaches the configuration set's EventBridge event destination, so the rule's Lambda target is never invoked

> _Note: this issue was identified against the code base at commit `fbef683f21970efb4631f2658975dd3b2c5af139`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendEmail

## Description
Cause 1 of 1: a successful SendEmail under a configuration set whose event destination is EventBridge with the DELIVERY event type published no event at all, so the EventBridge rule matching detail-type 'Email Delivered' never fires and its Lambda target is never invoked.
AWS: recorded behavior only — the sesv2 Smithy model defines SendEmail (POST /v2/email/outbound-emails) and CreateConfigurationSetEventDestination's EventBridgeDestination/MatchingEventTypes, but not the event payload delivered to EventBridge. The behavior exercised: a send to the mailbox simulator address success@simulator.amazonses.com under that configuration set publishes an event with source 'aws.ses' and detail-type 'Email Delivered' whose detail.eventType is 'Delivery' and whose detail.mail.destination contains the recipient; a rule on {source: [aws.ses], detail-type: [Email Delivered]} with a Lambda target (plus the lambda:InvokeFunction grant for events.amazonaws.com) invokes that function with the event.
Emulator: no event is ever delivered and the target function is never invoked — the relay queue stays empty and the check is killed at the harness budget with 'Check timed out after 60.0s'. Nothing is refused; the delivery-event publication simply does not happen.
Why: email event publishing is how a client learns the fate of a message. With no Delivery event, delivery dashboards, per-message auditing and any automation keyed on 'Email Delivered' never run, and a delivered message is indistinguishable from one that was never processed. This is the DELIVERY event type specifically: the open issue 20261101T210635Z-lambda-sesv2-sesv2-send-bounce-events-never-reach.md reports the Send and Bounce event types and carries only those two checks' replay keys, so this check's event type is not covered by it; a fixer closing that issue must re-replay this check too, because DELIVERY is a separate entry in MatchingEventTypes and can remain unpublished after Send/Bounce are fixed.
Fix: the SESv2 send path's event publication to the configuration set's EventBridge event destination — the per-event-type gate that must emit a 'Delivery'/'Email Delivered' aws.ses event once a send to a deliverable (simulator) recipient completes — plus the EventBridge rule dispatch that invokes the Lambda target for that event.
Traces: lambda__sesv2.sesv2-eventbridge-delivery-event-lambda-positive.

## Evidence
Replay of suite simparity-behavioral-aws-v1; verdict [mismatch], Error: Check timed out after 60.0s.
Expected (AWS): after SendEmail(ConfigurationSetName=<cs with an EventBridge destination for DELIVERY>, Destination=success@simulator.amazonses.com), at least one relayed event with source 'aws.ses', detail-type 'Email Delivered', detail.eventType 'Delivery' and the recipient present in detail.mail.destination; the rule's Lambda target invoked with it.
Actual (emulator): the relay queue receives nothing within the 180 s wait the check asks for and the harness kills the check at 60 s — no event published, no Lambda invocation. The setup calls (verified sender discovery, SQS queue, IAM role, CreateFunction, configuration set with the EventBridge destination, PutRule/PutTargets, AddPermission) all completed; only the event never arrives.

## Steps to Reproduce
1. Obtain a verified sending identity and create a configuration set with an EventBridge event destination whose MatchingEventTypes is ['DELIVERY'].
2. Create an EventBridge rule with pattern {"source":["aws.ses"],"detail-type":["Email Delivered"]}, a Lambda target that relays its event to an SQS queue, and lambda AddPermission for events.amazonaws.com on that rule ARN.
3. sesv2.send_email(ConfigurationSetName=<cs>, FromEmailAddress=<verified>, Destination={ToAddresses:['success@simulator.amazonses.com']}, Content=...).
4. Poll the relay queue: AWS yields an aws.ses 'Email Delivered' event with detail.eventType 'Delivery'; the emulator yields nothing.
5. Re-replay lambda__sesv2.sesv2-eventbridge-delivery-event-lambda-positive.
