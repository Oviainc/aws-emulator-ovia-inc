Issue-Source: trace-sweep-divergence
Issue-Trace: sesv2-configuration-set-publishes-delivery-event-to-sns
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sesv2-configuration-set-publishes-delivery-event-to-sns
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2|SendEmail|2xx->4xx|$.MessageId
Issue-Key: sesv2|SendEmail|2xx->4xx|status
Issue-Commit: 0a7cae03cb1d4210d38997f5682a54b52a42bce2

# sesv2 SendEmail refuses the configuration-set send with 400 and mints no MessageId

> _Note: this issue was identified against the code base at commit `0a7cae03cb1d4210d38997f5682a54b52a42bce2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendEmail

## Description
One cause, on service token sesv2.

Cause 1 of 1: SendEmail answers HTTP 400 (a client-error refusal) and no MessageId where AWS accepts the message with 200, for a send whose every precondition the same fixture already established successfully (fixture: sesv2-configuration-set-publishes-delivery-event-to-sns#task#8).

AWS: Smithy (sesv2): SendEmail is POST /v2/email/outbound-emails with success code 200 and output SendEmailResponse{MessageId: OutboundMessageId}; its modeled client errors are AccountSuspendedException, BadRequestException, LimitExceededException, MailFromDomainNotVerifiedException, MessageRejected and SendingPausedException (all HTTP 400), NotFoundException (404) and TooManyRequestsException (429). Recorded behavior: at this step the recording answers 200 with a MessageId (the value is per-run and shape-asserted only, so no literal id from the recording is a requirement). Every preceding step of the fixture — the SNS/SQS setup, the configuration set and its SNS event destination, and the sending identity — answered exactly as recorded, so at the moment of the send the account holds a verified sending identity and an existing, sending-enabled configuration set, and AWS's answer for that request is an accepted message: 200 plus a freshly minted MessageId. A 400 is correct only for a request AWS actually refuses (an unverified From identity, an empty Destination, an empty body, a paused account or paused configuration set).

Emulator: the step answers status 400 instead of 200 and the response carries no MessageId at all ($.MessageId: missing, expected '<shaped-not-asserted>', got None). Because the recorded response for an accepted send has no error members, the replay compared only the status and MessageId, so the refusal's error code and message text were not captured by this step — the observable divergence is the status class (400 where 200 is due) and the absent MessageId.

Hypothesis (neither the model nor the recording states which of the handler's gates fired): the refusal is one of the pre-send gates in the SendEmail handler, the From-identity verification gate being the most likely — services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/send_email.rs looks the sender up with store.sending_identity(&from) and requires identity.verified_for_sending, otherwise returning MessageRejected with 'Email address is not verified. The following identities failed the check in region <REGION>: <identities>' (fixed sentence, with the upper-cased request region and the failing address taken from the request and state). The other 400 arms in that file are BadRequestException 'Missing required header 'From'.', BadRequestException 'Destination must contain at least one of ToAddresses, CcAddresses or BccAddresses.', BadRequestException 'Body must contain either a Text or an Html part.' and SendingPausedException for the account or the named configuration set. Whichever fired, it fired for a request AWS accepts.

Why: a client that creates a configuration set, attaches an event destination and sends through it gets a client error back and no message id, so the message is never logged, no Send/Delivery event is ever published, and the whole downstream delivery leg of the scenario is unreachable. A 4xx also tells the SDK the caller's request was wrong, so the caller retries nothing and 'fixes' a request that was already valid.

Fix: the pre-send gates of the SendEmail handler (services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/send_email.rs, the sending_identity/verified_for_sending check and the account/configuration-set sending switches) must admit a send whose From identity this account has verified and whose named configuration set exists and is sending-enabled, and return 200 with a minted MessageId. Related open issues cover neighbouring symptoms and none carries this fixture step's replay key, so none closes it: 20261108T003043Z-lambda-sesv2-7-net-new-causes-sesv2-enforces.md (Cause 2, 'SendEmail rejects verified identities') records the same refusal from the behavioral corpus; 20261102T025917Z-sesv2-sns-sqs-sendemail-publishes-no-event-to.md and 20261109T155235Z-sqs-events-published-to-an-eventbridgev2-bus-are.md cover the publish/delivery legs that follow this step.

Traces: sesv2-configuration-set-publishes-delivery-event-to-sns#task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture sesv2-configuration-set-publishes-delivery-event-to-sns, step task#8 (sesv2 SendEmail):
  status: expected 200, got 400
  $.MessageId: missing (expected '<shaped-not-asserted>', got None)

Expected: HTTP 200 with SendEmailResponse.MessageId present (shape-asserted). Actual: HTTP 400 with no MessageId, so nothing was accepted and nothing was logged.

Consequence observed in the same fixture (both steps already owned by 20261109T155235Z-sqs-events-published-to-an-eventbridgev2-bus-are.md, listed here only as the downstream effect): task#9 sqs GetQueueAttributes reports ApproximateNumberOfMessages '0' instead of '1' after 40 polls, and task#10 sqs ReceiveMessage returns an empty body instead of one Message.

Supporting code: the only HTTP-400 arms reachable in services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/send_email.rs are the BadRequestException arms (missing From, no recipient, no body part), the MessageRejected arm taken when store.sending_identity(&from) is absent or not verified_for_sending, and the SendingPausedException arms for the account switch and the named configuration set's switch.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay wire fixture sesv2-configuration-set-publishes-delivery-event-to-sns from trace set simparity-wire-aws-v1.
2. Let the fixture's setup run: create the SNS topic and SQS queue with the subscription, create the sesv2 configuration set, create its SNS event destination, and establish the sending identity for the From address — every one of these answers as recorded.
3. Step task#8: call sesv2 SendEmail with that From address, a Simple content body, one ToAddresses recipient and the configuration set's name.
4. Observe HTTP 400 and no MessageId in the body, where AWS answers HTTP 200 with a MessageId.
5. Inspect the 400's x-amzn-errortype and message to see which pre-send gate fired; the send must be accepted instead.
