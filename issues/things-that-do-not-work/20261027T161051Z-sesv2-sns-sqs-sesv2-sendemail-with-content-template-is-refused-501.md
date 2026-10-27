Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2__sns__sqs.templated-send-with-missing-variable-publishes-rendering-failure
Issue-Key: sesv2__sns__sqs|SendEmail|NotImplemented
Issue-Commit: f2505e07aa1dd14b6c340e2a3611fb80270bd1c6

# sesv2: SendEmail with Content.Template is refused 501 NotImplemented, so no templated send (and no asynchronous Rendering Failure) can ever happen

> _Note: this issue was identified against the code base at commit `f2505e07aa1dd14b6c340e2a3611fb80270bd1c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendEmail

## Description
One net-new root cause in the SESv2 send plane. The templated-send path of an otherwise implemented SendEmail handler is hard-coded to refuse.

=== Cause 1 of 1: SendEmail refuses every request whose Content selects Template ===

AWS: Smithy (services/service-com-amazonaws-sesv2-simpleemailservice-v2/smithy-model/com-amazonaws-sesv2-simpleemailservice-v2.json) binds SendEmail to POST /v2/email/outbound-emails with success code 200 and output {MessageId}. Its required input member Content targets EmailContent, a structure whose three members are Simple, Raw and Template; Template targets Template {TemplateName (len 1..), TemplateArn, TemplateContent, TemplateData (len 0..262144), Headers, Attachments}, documented as "An object that defines the email template to use for an email message, and the values to use for any message variables in that template", and the operation's own documentation names the templated form as a first-class message type: "Templated - A message that contains personalization tags. When you send this type of email, Amazon SES API v2 automatically replaces the tags with values that you specify." The modeled error set for SendEmail is exactly AccountSuspendedException (400), BadRequestException (400), LimitExceededException (400), MailFromDomainNotVerifiedException (400), MessageRejected (400), NotFoundException (404), SendingPausedException (400), TooManyRequestsException (429) — there is no 501 and no 'NotImplemented' code in the contract, so a templated send is either accepted 200 with a MessageId or refused with one of those eight. Recorded behavior (the grounding check): a templated SendEmail through a configuration set is accepted synchronously with a fresh MessageId EVEN WHEN TemplateData omits a variable the template body references — OutboundMessageId is documented as "It's possible for Amazon SES to accept a message without sending it ... or when you send a templated email that contains invalid personalization content" — and the rendering failure is reported LATER, asynchronously, as a 'Rendering Failure' event on the configuration set's event destinations, carrying mail.messageId equal to the MessageId that was returned, failure.templateName equal to the template named in the request, and a failure.errorMessage that names the missing variable. Supplying the variable produces a Send event and no Rendering Failure.

Emulator: the request never reaches any of that. services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/send_email.rs dispatches Content in message_parts(), and the (None, None, Some(_template)) arm returns crate::scaffolded::Scaffolded::not_implemented("SendEmail with templated content"), i.e. HTTP 501 with error code 'NotImplemented' and message "operation SendEmail with templated content is scaffolded but not implemented in this emulator". The Simple and Raw arms of the same function are fully implemented (sender verification, recipient checks, 24h quota, MessageId minting, outbound log), so the refusal is specific to the templated form. The fixed text of the emulator's message is "operation " ... " is scaffolded but not implemented in this emulator" with the scaffold label "SendEmail with templated content" interpolated; no part of it is AWS wording.

Why: a client cannot send a templated email at all. Every library and application that composes mail from a stored SES template gets an HTTP status and an error code outside SendEmail's modeled error set, which SDK error handling cannot classify (it is not retryable, not throttling, not a validation error), so it surfaces as an opaque failure rather than as MessageRejected/NotFoundException/BadRequestException. It also makes SES's whole asynchronous rendering contract unobservable: the only signal AWS gives for a bad template/data pair is the later 'Rendering Failure' event, and because the synchronous call is refused, a client can never distinguish "accepted, failed to render" from "rejected" — the two have opposite handling (the first needs event-driven remediation, the second a request fix).

Fix: the operation's handler and the template state it must read — services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/send_email.rs (the Template arm of message_parts() and the handle() body that follows it) over the email-template store in services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/state.rs: resolve Content.Template.TemplateName (or TemplateArn) against the templates CreateEmailTemplate stores, refuse an unknown one with the modeled NotFoundException (404) rather than 501, otherwise substitute the TemplateData key/value pairs into the stored Subject/Text/Html, accept the send, mint and return a MessageId, and record it in the outbound log exactly as the Simple arm does — including when a referenced variable is absent from TemplateData, which must NOT make the synchronous call fail. Emitting the resulting 'Rendering Failure' event additionally needs the event-publication path described in the open issue 20261026T065823Z-sesv2-sns-sqs-ses-publishes-no-events-to.md (Cause 1 there), so confirming this cause end-to-end depends on that issue; that issue's own Cause 2 names CreateEmailTemplate's 501 stub, which in this replay now succeeds — the check gets one call further and dies on SendEmail instead, which is why this is a distinct, net-new cause and not covered by it.

Traces: sesv2__sns__sqs.templated-send-with-missing-variable-publishes-rendering-failure.

## Evidence
Behavioral suite replay, surviving (neither already-filed nor owned) failure:

[error] sesv2__sns__sqs.templated-send-with-missing-variable-publishes-rendering-failure
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the SendEmail operation: operation SendEmail with templated content is scaffolded but not implemented in this emulator

Expected (AWS): HTTP 200 with a MessageId for the templated send, both for the positive control (TemplateData supplies {"name": ...}) and for the probe (TemplateData is "{}", omitting the variable the template body references); the probe's failure then arrives asynchronously as a 'Rendering Failure' event on the configuration set's SNS destination whose mail.messageId equals the returned MessageId, whose failure.templateName equals the created template's name, and whose failure.errorMessage names the missing variable.
Actual (emulator): HTTP 501, error code 'NotImplemented', message "operation SendEmail with templated content is scaffolded but not implemented in this emulator". 501/'NotImplemented' is outside SendEmail's modeled error set (AccountSuspendedException, BadRequestException, LimitExceededException, MailFromDomainNotVerifiedException, MessageRejected, NotFoundException, SendingPausedException, TooManyRequestsException).

Emulator source: services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/send_email.rs — message_parts(), arm (None, None, Some(_template)) returns Scaffolded::not_implemented("SendEmail with templated content") (lines ~440-445); the Simple arm (lines ~416-432) and the Raw arm (lines ~435-439) are implemented, and the rest of handle() (sender verification, quota, MessageId minting, outbound log) is reached only by them.

Note on already-filed neighbours: the open issue 20261026T065823Z-sesv2-sns-sqs-ses-publishes-no-events-to.md lists this same check, but its recorded key for it is CreateEmailTemplate/NotImplemented — that setup call now succeeds, so this SendEmail refusal is a different, later failure point and the suite did not mark the check as owned.

## Steps to Reproduce
1. Create an SNS topic and an SQS queue subscribed to it with RawMessageDelivery=true, and a queue policy allowing sns.amazonaws.com:SendMessage from that topic.
2. sesv2 CreateConfigurationSet, then CreateConfigurationSetEventDestination with EventDestination={Enabled: true, MatchingEventTypes: ['SEND','RENDERING_FAILURE'], SnsDestination: {TopicArn}}.
3. sesv2 CreateEmailTemplate with TemplateContent={Subject: 'sim2real {{name}}', Text: 'Hello {{name}}, this is a rendering probe.'} (this now succeeds).
4. From a verified sender, call sesv2 SendEmail with Content={'Template': {'TemplateName': <that template>, 'TemplateData': '{"name": "control"}'}} and ConfigurationSetName=<that set>. Expect HTTP 200 with a MessageId; observe HTTP 501 'NotImplemented' — "operation SendEmail with templated content is scaffolded but not implemented in this emulator".
5. Repeat with TemplateData='{}' (omitting the referenced variable). Expect HTTP 200 with a fresh MessageId and, later, a 'Rendering Failure' event on the queue naming that MessageId, the template and the missing variable; observe the same 501 refusal instead.

Replay check: sesv2__sns__sqs.templated-send-with-missing-variable-publishes-rendering-failure.
