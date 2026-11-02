Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2__sns__sqs.event-destination-disabled-publishes-no-events
Issue-Key: sesv2__sns__sqs.event-destination-disabled-publishes-no-events|_drain_until#26|Timeout|for m in sqs.receive_message(
Issue-Commit: 9010f89773c9fbf121fd2271b9167862206a1831

# sesv2__sns__sqs: SendEmail publishes no event to the configuration set's SNS event destination, so the Enabled gate cannot be observed in either direction

> _Note: this issue was identified against the code base at commit `9010f89773c9fbf121fd2271b9167862206a1831`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendEmail

## Description
One net-new root cause in the SESv2 -> SNS event-publication slice.

Cause 1 of 1: an accepted SendEmail never publishes an SES event to the named configuration set's SNS event destination, so the destination's `Enabled` flag is never read and both legs of the gate (disabled -> nothing, re-enabled -> the Send event) look identical.

AWS: Smithy (services/service-com-amazonaws-sesv2-simpleemailservice-v2/smithy-model/com-amazonaws-sesv2-simpleemailservice-v2.json) models SendEmail (POST /v2/email/outbound-emails) answering 200 with MessageId, and EventDestinationDefinition.SnsDestination -> SnsDestination{TopicArn (required)} documented as "An object that defines an Amazon SNS destination for email events. You can use Amazon SNS to send notifications when certain email events occur"; EventDestinationDefinition.Enabled is the switch that turns the destination on and off, and MatchingEventTypes selects which events it receives. Recorded behavior: for a send through a configuration set holding an Enabled=true SnsDestination whose MatchingEventTypes include SEND, SES publishes one JSON event object per matching event to the topic — eventType "Send", mail.messageId equal to the MessageId that SendEmail returned, mail.source equal to the sender. With Enabled=false SES publishes NOTHING for the same send, while SendEmail still returns a real MessageId; flipping the SAME destination back with UpdateConfigurationSetEventDestination(Enabled=true) makes the NEXT send publish its Send event, and that event must name the second send's MessageId, not the earlier one. The pair is the requirement: the absence is only meaningful because the re-enabled leg proves the wiring was live.

Emulator: nothing is ever published, so neither leg is observable. services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/send_email.rs settles the clock, resolves ListManagementOptions, gates on the account-level SendingEnabled switch, on the named configuration set's existence and its own sending switch, on the sender identity and the quota, mints a MessageId and appends to the in-memory outbound log — it never reads the configuration set's event destinations, never consults Enabled or MatchingEventTypes, and has no publish path to the SNS slice. The check's negative leg (a 45s drain expecting nothing) is therefore satisfied vacuously; the positive-control leg re-enables the destination, sends again and drains for up to 120s for a 'Send' event that never arrives, and the suite reports "Check timed out after 60.0s". No status or error code diverges — SendEmail answers its modelled 200 with a MessageId — the divergence is the absent event.

Why: event publication is the only way a client learns what happened to a message (Send, Delivery, Bounce, Complaint, Rendering Failure). Against the emulator the topic and queue stay empty forever while the control plane reports a healthy destination, so no delivery accounting, bounce handling or header-fidelity logic can be exercised. And because Enabled is never read, a later implementation that publishes unconditionally would be wrong in the opposite direction — the disabled leg of this check is precisely what catches that.

Fix: SendEmail's handler plus the event-destination state it must read (services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/send_email.rs and the configuration-set / event-destination records in src/state.rs): on an accepted send, for each event destination of the named configuration set whose Enabled is true and whose MatchingEventTypes select the event, build the SES event object (eventType, mail.messageId from the minted MessageId, mail.source, mail.headers/commonHeaders from the message as sent) and deliver it to the destination — for an SnsDestination, by publishing to the named topic through the emulator's SNS slice — and publish nothing when Enabled is false.

Related filed work: 20261026T065823Z-sesv2-sns-sqs-ses-publishes-no-events-to.md describes this same inert publication path and lists this check among its trace ids, but the replay report does NOT mark this failure as owned — the key that issue carries for this check (_drain_once frame) is not the key this replay produced, so the failure is filed here with its current key rather than left uncovered. A fixer closing that issue should re-replay this check too. 20261029T183635Z-sesv2-sns-sqs-a-send-to-the-mailbox.md and 20261026T065823Z-sesv2-sendbulkemail-stub.md cover neighbouring scenarios (simulator Delivery/Bounce events, SendBulkEmail) and not this check.

Traces: sesv2__sns__sqs.event-destination-disabled-publishes-no-events.

## Evidence
Failing check: [mismatch] sesv2__sns__sqs.event-destination-disabled-publishes-no-events — "Error: Check timed out after 60.0s".

Sequence replayed, all calls 2xx: route53 ListHostedZones/ListTagsForResource to find the DNS-baseline domain and sesv2 GetEmailIdentity confirming it is verified for sending; CreateTopic; CreateQueue + GetQueueAttributes(QueueArn) + SetQueueAttributes(Policy allowing sns.amazonaws.com sqs:SendMessage with ArnEquals aws:SourceArn = the topic); Subscribe(Protocol='sqs', Endpoint=<queue arn>, Attributes={RawMessageDelivery:'true'}); CreateConfigurationSet; CreateConfigurationSetEventDestination(EventDestination={Enabled:false, MatchingEventTypes:['SEND','DELIVERY'], SnsDestination:{TopicArn}}); SendEmail through the set -> 200 with a MessageId longer than 10 chars (as required); a 45s ReceiveMessage drain of the queue -> empty, as expected; UpdateConfigurationSetEventDestination(same destination, Enabled:true) -> 2xx; SendEmail again -> 200 with a second MessageId; then a drain of up to 120s for an event whose eventType is 'Send'.

Expected (positive control): one JSON event on the queue with eventType 'Send' whose mail.messageId equals the MessageId the SECOND SendEmail returned.
Actual: the queue never receives any object carrying an eventType; the drain runs to its own deadline and the per-check budget expires at 60s.

Emulator source: src/ops/send_email.rs — the handler's gates and accounting (clock settle, ListManagementOptions resolution, account SendingEnabled, configuration-set existence + sending switch, sender verification, quota) are followed by minting the MessageId and appending the SentMessage; there is no read of the configuration set's event destinations and no publish anywhere in the crate, so Enabled is never consulted.

## Steps to Reproduce
1. CreateTopic(t); CreateQueue(q); GetQueueAttributes(q,['QueueArn']); SetQueueAttributes(q, Policy allowing Principal {Service: sns.amazonaws.com} sqs:SendMessage on q with Condition ArnEquals aws:SourceArn = t); Subscribe(TopicArn=t, Protocol='sqs', Endpoint=<q arn>, Attributes={RawMessageDelivery:'true'}).
2. CreateConfigurationSet(cs); CreateConfigurationSetEventDestination(cs, EventDestinationName='gate', EventDestination={Enabled:false, MatchingEventTypes:['SEND','DELIVERY'], SnsDestination:{TopicArn:t}}).
3. SendEmail(FromEmailAddress=<verified sender>, Destination={ToAddresses:['success@simulator.amazonses.com']}, Content.Simple, ConfigurationSetName=cs) -> expect 200 with a MessageId. Drain q for ~45s -> expect NOTHING (correct, and the emulator also shows nothing).
4. UpdateConfigurationSetEventDestination(cs, 'gate', EventDestination={Enabled:true, MatchingEventTypes:['SEND','DELIVERY'], SnsDestination:{TopicArn:t}}).
5. SendEmail again through cs -> 200 with a second MessageId. Drain q for up to 120s expecting an event with eventType 'Send' and mail.messageId equal to that second MessageId.
6. Observe the queue stays empty: the emulator publishes no SES event for any send, so the positive control never settles and the check is cut off at 60s.

Re-replay check: sesv2__sns__sqs.event-destination-disabled-publishes-no-events.
