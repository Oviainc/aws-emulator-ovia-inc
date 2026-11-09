Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ses__sesv2.send-to-unverified-recipient-refused-both-apis-then-simulator-accepted
Issue-Key: ses__sesv2.send-to-unverified-recipient-refused-both-apis-then-simulator-accepted|_refusal#8|AssertionError|raise AssertionError(f"SendEmail to an unverified, non-simulator recipient must be refused; got {result!r}")
Issue-Commit: 5b2a9eedb8fa287cac2318fd09ae07f52de03ab6

# ses__sesv2: SendEmail to an unverified, non-simulator recipient is ACCEPTED with 200 and a MessageId instead of being refused with 400 MessageRejected

> _Note: this issue was identified against the code base at commit `5b2a9eedb8fa287cac2318fd09ae07f52de03ab6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendEmail

## Description
One net-new root cause, grounded in the single failing ses__sesv2 check. The replay marks it neither "ALREADY FILED" nor "OWNED by", so it is filed here. No open issue covers it: a keyword search for "unverified" returns only an unrelated MWAA issue.

Cause 1 of 1: in the SES sandbox, SendEmail from a verified sender to a recipient that is neither a verified identity nor a mailbox-simulator address is accepted — the emulator answers HTTP 200 with a MessageId — where AWS refuses the send with 400 MessageRejected. The sandbox recipient-verification gate is absent from the sending path.
Failing check id: ses__sesv2.send-to-unverified-recipient-refused-both-apis-then-simulator-accepted.
AWS: the refusal is modelled; its message text is published AWS behavior. Smithy (com.amazonaws.sesv2): SendEmail POST /v2/email/outbound-emails, input {Content @required (EmailContent), FromEmailAddress (EmailAddress), Destination, ConfigurationSetName, ...}, output {MessageId (OutboundMessageId)}, success code 200; modeled error MessageRejected, errorKind client, httpError 400 ("The message can't be sent because it contains invalid content."). Smithy (com.amazonaws.ses): SendEmail — input {Source @required (Address), Destination @required, Message @required}, output {MessageId @required}; modeled error MessageRejected, errorKind client, httpError 400, awsQueryError code MessageRejected, httpResponseCode 400. Recorded behavior: while the account is in the SES sandbox, every destination address must itself be a verified identity (or the mailbox simulator), so a send to an address on a domain no account can verify is refused by BOTH APIs with HTTP 400, Error.Code `MessageRejected` and Error.Message `Email address is not verified. The following identities failed the check in region <REGION>: <failing addresses>`. Fixed text of that message: the literal `Email address is not verified. The following identities failed the check in region ` and the literal `: ` separator. Variable parts, from the request and the emulator's own configuration: <REGION> is the signing region upper-cased (the check builds it as ctx.region.upper()), and the trailing list is the destination address(es) from the request that failed the check, comma-separated. The paired positive is equally recorded: the SAME sender reaching success@simulator.amazonses.com succeeds with 200 and a non-empty string MessageId.
Emulator: sesv2 SendEmail answers HTTP 200 with {'MessageId': '010001a1c0fec3c9-0000000000000001-000000'} and content-type application/json for a Destination.ToAddresses naming an address on the reserved `recipient.test` domain — no identity is verified for it and none can be. No MessageRejected is raised, so the error code and message never exist to compare. Because the v2 assertion fails first, the v1 ses SendEmail half of the same rule was not exercised on this run; the gate is one shared sandbox rule and both APIs must enforce it identically.
Why: the sandbox recipient gate is the primary guard-rail every SES integration is written against — client code has an error branch for MessageRejected and test suites assert that sends to unverified recipients are blocked. On this emulator that branch is dead code and the assertion inverts: a caller believes an email was accepted for delivery to an arbitrary unverified address, so code paths that must verify a recipient before mailing it (double opt-in flows, bounce-handling, production-access gating) are never exercised and ship broken. It also makes the emulator silently more permissive than AWS, which is the exact class of divergence that turns a passing test suite into a production failure.
Fix: the recipient-verification gate in the sending path — the SendEmail handler's pre-flight validation in the sesv2 crate (services/service-com-amazonaws-sesv2-simpleemailservice-v2) and in the ses crate (services/service-com-amazonaws-ses-simpleemailservice), reading the identity state the verification operations write: while the account is in the sandbox, every address in Destination (To/Cc/Bcc) that is neither a verified identity (address or its domain) nor a *@simulator.amazonses.com mailbox-simulator address must make the call fail with 400 MessageRejected carrying the `Email address is not verified. The following identities failed the check in region <REGION>: <failing addresses>` message, while a send whose only destination is the mailbox simulator keeps returning 200 with a MessageId.
Traces: ses__sesv2.send-to-unverified-recipient-refused-both-apis-then-simulator-accepted.

## Evidence
Replayed behavioral suite simparity-behavioral-aws-v1: 95/132 passed (72.0%); per-service line `ses__sesv2           0/1 (0%) 1 failed`. The check is in the failing list as [mismatch] and is NOT marked "ALREADY FILED" or "OWNED by".

Failing detail (verbatim tail):
  File "check_ses__sesv2_send_to_unverified_recipient_refused_both_apis_then_simulator_accepted.py", line 74, in run
    err_v2 = _refusal(
  File "...", line 45, in _refusal
    raise AssertionError(f"SendEmail to an unverified, non-simulator recipient must be refused; got {result!r}")
  AssertionError: SendEmail to an unverified, non-simulator recipient must be refused; got {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '56', 'date': '...'}, 'RetryAttempts': 0}, 'MessageId': '010001a1c0fec3c9-0000000000000001-000000'}

Expected (AWS), for both sesv2 SendEmail and ses SendEmail with the same sender/recipient:
  HTTP 400; Error.Code == 'MessageRejected'; Error.Message == 'Email address is not verified. The following identities failed the check in region <REGION>: <recipient>' — fixed text 'Email address is not verified. The following identities failed the check in region ' + <REGION> + ': ' + the failing destination address(es) from the request.
Actual (emulator): sesv2 SendEmail returns HTTP 200 with a MessageId and no error at all. The v1 ses SendEmail call on the same rule was never reached because the v2 assertion failed first.

The recipient used is a per-run minted local part on the reserved `recipient.test` domain (an address no account can verify, so it is refused rather than mailed) and the MessageId/date above are per-run values — neither is part of the requirement. The requirement is the status, the code and the message template.

Not asserted here and not part of this issue: the specific MessageId format, request ids, or timestamps.

## Steps to Reproduce
1. Build and boot the emulator; point ses, sesv2 and route53 clients at it.
2. Obtain a verified sending domain <domain> (sesv2 GetEmailIdentity reports VerifiedForSendingStatus true for it) and pick sender = <local>@<domain>.
3. Pick recipient = <local>@recipient.test — an address on a reserved test domain that is NOT a verified identity and is not the mailbox simulator.
4. sesv2 SendEmail FromEmailAddress=sender, Destination={'ToAddresses': [recipient]}, Content={'Simple': {'Subject': {'Data': 'Recipient check'}, 'Body': {'Text': {'Data': 'this send must be refused'}}}}.
   Expected: HTTP 400, Error.Code 'MessageRejected', Error.Message 'Email address is not verified. The following identities failed the check in region <REGION>: <recipient>'.
   Observed: HTTP 200 with {'MessageId': '<some id>'} — no refusal.
5. ses (v1) SendEmail Source=sender, Destination={'ToAddresses': [recipient]}, Message={'Subject': {'Data': 'Recipient check'}, 'Body': {'Text': {'Data': 'this send must be refused'}}}.
   Expected: the same 400 MessageRejected with the same message.
6. Positive control, which must keep working: sesv2 SendEmail from the same sender with Destination={'ToAddresses': ['success@simulator.amazonses.com']} returns 200 with a non-empty string MessageId.
7. Or re-replay ses__sesv2.send-to-unverified-recipient-refused-both-apis-then-simulator-accepted.
