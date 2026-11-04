Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2__sns__sqs.account-suppression-from-a-hard-bounce-blocks-the-next-send
Issue-Key: sesv2__sns__sqs.account-suppression-from-a-hard-bounce-blocks-the-next-send|run#30|Timeout|time.sleep(5)
Issue-Commit: d220b5129ac92467b2bf094da95d24c404aff436

# sesv2__sns__sqs: a hard bounce never adds the destination to the account-level suppression list, so a later send to it is not blocked and produces no OnAccountSuppressionList bounce event (1 cause)

> _Note: this issue was identified against the code base at commit `d220b5129ac92467b2bf094da95d24c404aff436`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendEmail

## Description
One net-new root cause (Cause 1 of 1).

Cause 1 of 1: SESv2's account-level suppression list is never populated by a hard bounce, so GetSuppressedDestination keeps answering NotFoundException and the next send to that address is neither blocked nor reported as a suppression-list bounce.
  AWS: recorded behavior only (the Smithy model defines SendEmail / GetSuppressedDestination / SuppressedDestination{EmailAddress, Reason, LastUpdateTime, Attributes{MessageId, FeedbackId}} and NotFoundException, but not the auto-suppression rule, which AWS documents for the default account-level suppression list). The recording shows: a SendEmail through a configuration set to an address at simulator.amazonses.com that is not a documented simulator alias produces a deterministic "unknown user" HARD bounce; within ~180 s GetSuppressedDestination(EmailAddress=<target>) stops raising and returns SuppressedDestination with Reason == "BOUNCE" and Attributes.MessageId == the MessageId of the bounced send. Until it is listed, GetSuppressedDestination answers NotFoundException with message "Email address <target> does not exist on your suppression list." — fixed text "Email address " + " does not exist on your suppression list.", the only variable part being the address from the request. A SECOND SendEmail to the now-suppressed address is still ACCEPTED with a fresh MessageId, but it is never delivered: the configuration set's event destination publishes a Bounce event for it with bounce.bounceSubType == "OnAccountSuppressionList" and bouncedRecipients[0].diagnosticCode == "Amazon SES did not send the message to this address because it is on the suppression list for your account. For more information about removing addresses from the suppression list, see the Amazon SES Developer Guide at https://docs.aws.amazon.com/ses/latest/dg/sending-email-suppression-list.html" (fixed text; no per-run values). A control send to success@simulator.amazonses.com through the same configuration set produces exactly a Send event.
  Emulator: the behavior never materialises. The check does not complete and the replay reports `Check timed out after 60.0s`; the only unbounded waits it contains are the 180 s poll of GetSuppressedDestination for the bounce-created entry and the drain of the 5 expected SES events, so nothing observable is produced on this path. Hypothesis: the emulator's SendEmail path neither simulates the simulator-mailbox hard bounce nor writes the account suppression entry, and consequently has no suppression check to short-circuit the second send.
  Why: a client that depends on SES's suppression semantics sees the opposite of AWS: addresses that hard-bounce stay sendable forever, GetSuppressedDestination/ListSuppressedDestinations/DeleteSuppressedDestination operate over an always-empty list, and a send that AWS silently drops with an OnAccountSuppressionList bounce looks like a successful delivery — reputation-protection logic built on these signals never triggers.
  Fix: the SESv2 send path and suppression-list state in the sesv2 service crate (services/service-com-amazonaws-sesv2-simpleemailservice-v2) — hard-bounce simulation for simulator.amazonses.com addresses must add the account suppression entry (Reason BOUNCE, Attributes.MessageId of the bounced send), SendEmail must consult the account suppression list and emit a Bounce with bounceSubType OnAccountSuppressionList and the documented diagnosticCode instead of attempting delivery, and GetSuppressedDestination must answer from that state.
  Related open issue that does NOT carry this check's replay key: 20261026T065823Z-sesv2-sns-sqs-ses-publishes-no-events-to.md (no SES events reach an SNS event destination at all). This cause is the suppression-list state machine itself; a fixer must make both the entry and the OnAccountSuppressionList bounce exist, not only the event transport.
  Traces: sesv2__sns__sqs.account-suppression-from-a-hard-bounce-blocks-the-next-send (mismatch, timed out at the harness cap).

## Evidence
Cause 1 of 1 — expected vs actual.
  Expected (AWS, recorded): SendEmail(FromEmailAddress=<verified domain sender>, ToAddresses=[<unique>@simulator.amazonses.com], ConfigurationSetName=<cs>) -> MessageId M1. Within 180 s GetSuppressedDestination(EmailAddress=<target>) returns SuppressedDestination{Reason: "BOUNCE", Attributes: {MessageId: M1}}; before that it raises NotFoundException "Email address <target> does not exist on your suppression list.". A second SendEmail to the same address returns a distinct MessageId M2 and yields a Bounce event with bounceSubType "OnAccountSuppressionList" and the documented suppression diagnosticCode; the control send to success@simulator.amazonses.com yields exactly a Send event.
  Actual (emulator): no suppression entry ever appears and none of the 5 expected SES events is observed; the check never reaches the Reason/MessageId/bounceSubType/diagnosticCode assertions and is reported as `Check timed out after 60.0s`.

## Steps to Reproduce
1. Create an SNS topic plus an SQS queue subscribed with RawMessageDelivery=true (queue policy allowing sns.amazonaws.com).
2. sesv2:CreateConfigurationSet(<cs>) and sesv2:CreateConfigurationSetEventDestination(<cs>, EventDestination={Enabled:true, MatchingEventTypes:[SEND,BOUNCE], SnsDestination:{TopicArn:<topic>}}).
3. sesv2:SendEmail from a verified sender in <cs> to a unique <id>@simulator.amazonses.com address (a non-alias simulator mailbox -> deterministic hard bounce); keep the returned MessageId M1.
4. Poll sesv2:GetSuppressedDestination(EmailAddress=<target>) for up to 180 s. AWS: NotFoundException "Email address <target> does not exist on your suppression list." until the entry appears with Reason BOUNCE and Attributes.MessageId == M1. Emulator: it never appears.
5. sesv2:SendEmail again to the same address -> accepted with a distinct MessageId M2; then drain the queue. AWS: a Bounce event for M2 with bounceSubType "OnAccountSuppressionList" and the documented diagnosticCode. Emulator: nothing.
  Replay: sesv2__sns__sqs.account-suppression-from-a-hard-bounce-blocks-the-next-send (allow its full declared budget, not the 60 s suite cap).
