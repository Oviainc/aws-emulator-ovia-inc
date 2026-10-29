Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2__sns__sqs.send-email-to-success-simulator-returns-message-id-and-a-delivery-event sesv2__sns__sqs.send-to-bounce-and-complaint-simulator-publishes-documented-event-shapes
Issue-Key: sesv2__sns__sqs.send-email-to-success-simulator-returns-message-id-and-a-delivery-event|_drain_until#21|Timeout|for m in sqs.receive_message(
Issue-Key: sesv2__sns__sqs.send-to-bounce-and-complaint-simulator-publishes-documented-event-shapes|_drain_until#22|Timeout|for m in sqs.receive_message(
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# sesv2__sns__sqs: a send to the mailbox simulator publishes no event-destination notification, so neither the Delivery event nor the Bounce/Complaint shapes ever reach the SNS destination (1 cause)

> _Note: this issue was identified against the code base at commit `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendEmail

## Description
Cause 1 of 1: SendEmail returns (or is expected to return) a MessageId but the configuration set's SNS event destination receives nothing.
AWS: SendEmail through a configuration set that has an enabled event destination with MatchingEventTypes covering SEND/DELIVERY/BOUNCE/COMPLAINT publishes one SNS notification per event to the destination topic. A send to success@simulator.amazonses.com produces a Delivery event; sends to bounce@simulator.amazonses.com and complaint@simulator.amazonses.com produce Bounce and Complaint events whose documented shapes carry eventType plus the mail block (messageId, source, destination, commonHeaders) and the bounce/complaint block (bounceType/bounceSubType/bouncedRecipients, complaintFeedbackType/complainedRecipients). Recorded behavior only for the exact timing; the event shapes are AWS-documented.
Emulator: both checks run to the harness cap and are terminated at 60 s - no notification of any kind is published to the SNS event destination, so neither the returned MessageId can be correlated to a Delivery event nor can the bounce/complaint shapes be inspected.
Why: the mailbox simulator is the only way to exercise deliverability handling without sending real mail; with no events published, a client's bounce/complaint processing (suppression, reputation handling, retry logic) has nothing to consume and cannot be developed or regression-tested at all.
Fix: the SendEmail/SendBulkEmail handler's event-emission path - resolve the configuration set's event destinations, honour MatchingEventTypes and the simulator recipient semantics, and publish the documented event JSON to the SNS destination. The open issue 20261029T160438Z-sesv2-sns-sqs-the-ses-sending-surface-is.md owns four sibling checks of this surface but does not carry these two replay keys, so they are filed here.
Traces: sesv2__sns__sqs.send-email-to-success-simulator-returns-message-id-and-a-delivery-event, sesv2__sns__sqs.send-to-bounce-and-complaint-simulator-publishes-documented-event-shapes (both: "Check timed out after 60.0s").

## Evidence
Expected: a Delivery event for the success simulator and documented Bounce/Complaint events for the bounce/complaint simulators, each published to the configuration set's SNS event destination and visible on the subscribed SQS queue; actual: the queue stays empty and the harness terminates both checks with "Error: Check timed out after 60.0s".

## Steps to Reproduce
1. CreateConfigurationSet; CreateConfigurationSetEventDestination with an SnsDestination topic and MatchingEventTypes covering SEND/DELIVERY/BOUNCE/COMPLAINT, Enabled=true; subscribe an SQS queue to that topic.
2. SendEmail with ConfigurationSetName set and Destination=success@simulator.amazonses.com, then bounce@simulator.amazonses.com and complaint@simulator.amazonses.com.
3. Poll the queue: no Delivery, Bounce or Complaint notification ever arrives.
