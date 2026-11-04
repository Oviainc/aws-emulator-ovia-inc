Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2__sns__sqs.send-email-to-success-simulator-returns-message-id-and-a-delivery-event sesv2__sns__sqs.send-raw-email-preserves-custom-headers-exactly-in-the-published-event sesv2__sns__sqs.send-to-bounce-and-complaint-simulator-publishes-documented-event-shapes
Issue-Key: sesv2__sns__sqs.send-email-to-success-simulator-returns-message-id-and-a-delivery-event|run#28|KeyError|assert delivery["delivery"]["smtpResponse"] == "250 Ok", (
Issue-Key: sesv2__sns__sqs.send-raw-email-preserves-custom-headers-exactly-in-the-published-event|run#35|KeyError|assert common["messageId"] == message_id, (
Issue-Key: sesv2__sns__sqs.send-to-bounce-and-complaint-simulator-publishes-documented-event-shapes|run#38|KeyError|assert complaint["userAgent"] == "Amazon SES Mailbox Simulator", (
Issue-Commit: 685d7dc1b36320cc963844050fe238af8df0451a

# sesv2__sns__sqs: published email-event documents omit documented members — Delivery.smtpResponse, mail.commonHeaders.messageId and Complaint.userAgent (3 causes)

> _Note: this issue was identified against the code base at commit `685d7dc1b36320cc963844050fe238af8df0451a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendEmail

## Description
This issue carries 3 root causes, each grounded in its own failing behavioral check. All three checks now get their notification delivered to the configuration set's SNS->SQS event destination and pass every earlier assertion, then fail with a KeyError on ONE member the published document does not contain. The OPEN issue 20261029T183635Z-sesv2-sns-sqs-a-send-to-the-mailbox.md ("a send to the mailbox simulator publishes no event-destination notification") does NOT cover these: the events do arrive now, and what diverges is the content of the documents. The events are not in the Smithy model (the event document is a JSON payload SES publishes, not a modelled shape), so the requirement below is AWS's documented email-event shape plus the recorded behavior the checks encode.

Cause 1 of 3 — the Delivery event's `delivery` object has no `smtpResponse`.
AWS: recorded behavior and the SES email-event documentation — a Delivery event's `delivery` object carries `timestamp`, `recipients`, `processingTimeMillis`, `reportingMTA` and `smtpResponse`, the SMTP reply the receiving side gave. For the mailbox simulator's success address success@simulator.amazonses.com the reply is deterministic and its fixed text is exactly "250 Ok" (SES itself answers for that mailbox, so nothing per-run appears in it).
Emulator: the Delivery document is built as json!({"timestamp": ..., "recipients": [...]}) only — services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/state/event_publication/notification.rs lines 176-186; the module doc (lines 16-18) states the omission as a deliberate choice ("no processing time, no reporting MTA and no SMTP reply, because no mail server answered one"). The check reads delivery["delivery"]["smtpResponse"] and raises KeyError: 'smtpResponse' after recipients already matched.
Why: a client that keys off smtpResponse (or processingTimeMillis / reportingMTA) to tell a plain acceptance from a real delivery, or to log the receiving server's reply, sees the member missing and either crashes or treats every delivery as unknown. The mailbox simulator exists precisely so that this value is deterministic, so it is reproducible here rather than un-knowable.
Fix: the Delivery notification builder in src/state/event_publication/notification.rs, with the simulator's per-mailbox answer coming from src/state/event_publication/outcome.rs (where MAILBOX_SIMULATOR_DOMAIN and the deterministic reactions already live).
Traces: sesv2__sns__sqs.send-email-to-success-simulator-returns-message-id-and-a-delivery-event.

Cause 2 of 3 — `mail.commonHeaders` has no `messageId` for a raw (MIME) send.
AWS: recorded behavior — the published document's mail.commonHeaders carries `messageId`, and it equals the MessageId SendEmail returned for that send (the check asserts common["messageId"] == the returned MessageId; its variable part is that id, the member name is fixed).
Emulator: commonHeaders is assembled ONLY from header lines the caller actually wrote, looking for COMMON_HEADER_NAMES = ["Subject", "Message-ID", "Date"] (notification.rs line 53, common_headers() lines 281-307). A raw MIME body that carries no Message-ID header therefore yields a commonHeaders object with no messageId member at all, and the check raises KeyError: 'messageId' after headers order/casing, the custom header, Subject, Content-Type and commonHeaders.subject all matched.
Why: SES stamps the accepted message with its own message id; a consumer correlating an event back to the SendEmail response through commonHeaders.messageId (a common pattern, because mail.messageId and commonHeaders.messageId are expected to agree) finds the member absent for every raw send and cannot correlate.
Fix: the commonHeaders builder in src/state/event_publication/notification.rs — the accepted message's own id must populate commonHeaders.messageId whether or not the caller wrote a Message-ID header.
Traces: sesv2__sns__sqs.send-raw-email-preserves-custom-headers-exactly-in-the-published-event.

Cause 3 of 3 — the Complaint event's `complaint` object has no `userAgent`.
AWS: recorded behavior and the SES complaint-event documentation — a Complaint event's `complaint` object carries `complainedRecipients`, `timestamp`, `feedbackId`, `complaintFeedbackType` and `userAgent`, the name of the agent that reported the complaint. For complaint@simulator.amazonses.com that value's fixed text is exactly "Amazon SES Mailbox Simulator".
Emulator: the complaint document is built with complainedRecipients, timestamp, feedbackId and (when known) complaintFeedbackType only — notification.rs lines 188-209 — so userAgent is never present. The check raises KeyError: 'userAgent' after the Bounce event's bounceType/bounceSubType/status/diagnosticCode and the complaint's complaintFeedbackType="abuse" all matched.
Why: complaint-processing pipelines record and filter on userAgent to tell a provider-generated feedback loop report from a test or simulator report; with the member missing, such a pipeline crashes or classifies every complaint as unknown-origin.
Fix: the Complaint notification builder in src/state/event_publication/notification.rs, with the simulator-specific value carried on the recipient reaction in src/state/event_publication/outcome.rs.
Traces: sesv2__sns__sqs.send-to-bounce-and-complaint-simulator-publishes-documented-event-shapes.

## Evidence
Cause 1 — expected: the Delivery event published to the configuration set's SNS->SQS destination contains delivery.smtpResponse == "250 Ok" for success@simulator.amazonses.com (alongside recipients and timestamp). Actual: `KeyError: 'smtpResponse'` at check line 164 (`assert delivery["delivery"]["smtpResponse"] == "250 Ok"`); the delivery object holds only timestamp and recipients.

Cause 2 — expected: for a Content.Raw send, the Send event's mail.commonHeaders.messageId equals the MessageId SendEmail returned. Actual: `KeyError: 'messageId'` at check line 193 (`assert common["messageId"] == message_id`); commonHeaders carries from/to/subject (built only from the caller's own header lines; COMMON_HEADER_NAMES in notification.rs line 53) and no messageId because the raw MIME carried no Message-ID header.

Cause 3 — expected: the Complaint event's complaint.userAgent == "Amazon SES Mailbox Simulator" for complaint@simulator.amazonses.com. Actual: `KeyError: 'userAgent'` at check line 184 (`assert complaint["userAgent"] == "Amazon SES Mailbox Simulator"`); the complaint object holds complainedRecipients, timestamp, feedbackId and complaintFeedbackType only.

In all three the earlier assertions passed, so the event reaches the queue with the right eventType and mail.messageId — only these members are absent.

## Steps to Reproduce
Common wiring (all three): sns:CreateTopic; sqs:CreateQueue and set a Policy allowing Service sns.amazonaws.com sqs:SendMessage with ArnEquals aws:SourceArn=<topic arn>; sns:Subscribe Protocol=sqs Endpoint=<queue arn> Attributes={RawMessageDelivery=true}; sesv2:CreateConfigurationSet; sesv2:CreateConfigurationSetEventDestination with EventDestination={Enabled:true, MatchingEventTypes:[...], SnsDestination:{TopicArn:<topic arn>}}.

1 (Delivery.smtpResponse): MatchingEventTypes=[SEND,DELIVERY]; sesv2:SendEmail FromEmailAddress=<verified domain address>, Destination.ToAddresses=[success@simulator.amazonses.com], Content.Simple, ConfigurationSetName=<set>. Drain the queue until eventType=="Delivery" and read event["delivery"] -> no "smtpResponse" key.

2 (commonHeaders.messageId): MatchingEventTypes=[SEND]; SendEmail with Content.Raw whose MIME header block is From/To/Subject/<custom header>/MIME-Version/Content-Type and no Message-ID. Drain until eventType=="Send" and read event["mail"]["commonHeaders"] -> no "messageId" key.

3 (complaint.userAgent): MatchingEventTypes=[BOUNCE,COMPLAINT]; SendEmail once to bounce@simulator.amazonses.com and once to complaint@simulator.amazonses.com. Drain until eventType=="Complaint" and read event["complaint"] -> complaintFeedbackType=="abuse" is present but "userAgent" is absent.
