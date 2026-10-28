Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ses.send-raw-email-unknown-configuration-set-fails
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: ses.send-raw-email-unknown-configuration-set-fails|_err#12|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded and returned {result!r}")
Issue-Commit: a7f4b46fd13e3042ad499c1371bfc738191ffa75

# ses SendRawEmail has no source-identity gate: a send from a Pending (never-verified) identity is accepted 200 with a MessageId instead of MessageRejected

> _Note: this issue was identified against the code base at commit `a7f4b46fd13e3042ad499c1371bfc738191ffa75`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendRawEmail

## Description
One net-new behavioral root cause in the SES v1 raw-send path. Related open issue 20261025T214303Z-ses-identity-verification-is-auto-completed-and-the.md reports the neighbouring defect that a v1-requested identity is reported 'Success' immediately; it does NOT carry this check's replay key and its causes are keyed on GetIdentityVerificationAttributes assertions, whereas the divergence below is observed on the SendRawEmail call itself (this check never reads the verification status). The fixed issue 20261026T065304Z-ses-ses-sendrawemail-stub.md (filename: 20261026T065304Z-ses-sendrawemail-stub.md) implemented SendRawEmail out of its 501 scaffold and carried this same check id; the call is no longer refused, so this is not a reopened refusal — the implemented handler is now over-permissive instead.

Cause 1 of 1: SendRawEmail does not refuse a message whose Source identity is not verified, so the whole unverified-sender refusal class is absent from the raw-send path.
AWS: VerifyEmailIdentity only *requests* verification — the address stays unverified until the confirmation link is clicked — and SendRawEmail from such an identity is refused. Smithy (com.amazonaws.ses#SimpleEmailService, awsQuery): SendRawEmail input {Source: Address, Destinations: AddressList, RawMessage required, ConfigurationSetName, FromArn/SourceArn/ReturnPathArn, Tags}, output {MessageId required}; modeled errors MessageRejected (awsQueryError code MessageRejected, httpError 400), ConfigurationSetDoesNotExist (code ConfigurationSetDoesNotExist, 400), ConfigurationSetSendingPausedException, AccountSendingPausedException, MailFromDomainNotVerifiedException. Source is documented as "The identity's email address". The refusal is 400 MessageRejected with the message "Email address is not verified. The following identities failed the check in region <REGION>: <identities>" — fixed text "Email address is not verified. The following identities failed the check in region ", then the upper-cased region the request was signed for, then ": ", then the comma-separated unverified identities taken from the request's Source/From. The region and the address in the recording are values filled into that template, never part of it.
Emulator: ses.verify_email_identity(EmailAddress=<addr>) followed immediately by ses.send_raw_email(Source=<addr>, Destinations=[<addr>], RawMessage={...}, ConfigurationSetName=<an existing configuration set>) answers HTTP 200 with {'MessageId': '0100...'} — no refusal at all. Replay: "AssertionError: expected the call to be refused, but it succeeded and returned {'MessageId': '0100e6a2ef83598c-...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}". Hypothesis: the handler either reads an identity record written as already-verified, or performs no Source lookup before minting a MessageId.
Why: a client that relies on AWS to refuse mail from an unverified address gets that mail accepted, so sandbox/identity enforcement is unfaithful and any test or guard built on the MessageRejected class silently passes. Because the gate never fires, the ordering this check exists to pin — ConfigurationSetDoesNotExist being raised AHEAD of the identity check, and a rejected send not moving GetSendQuota.SentLast24Hours nor creating the named configuration set — is unreachable and remains unverified against the emulator; expect those assertions to surface next once the identity gate is armed.
Fix: the source-identity gate in the v1 SendRawEmail handler and the identity record its state exposes (services/service-com-amazonaws-ses-simpleemailservice/src/ops/send_raw_email.rs plus the identity store it reads): resolve Source (and the raw message's From) against the account's identities, and refuse an identity that is not Success-verified with 400 MessageRejected and the message template above, ordered after the ConfigurationSetName existence check.
Traces: ses.send-raw-email-unknown-configuration-set-fails.

## Evidence
Expected: ses.send_raw_email(Source=<just-requested, Pending address>, Destinations=[same], RawMessage={'Data': a minimal text/plain MIME body}, ConfigurationSetName=<a configuration set created seconds earlier>) refused with ('MessageRejected', 'Email address is not verified. The following identities failed the check in region US-WEST-2: <that address>') — HTTP 400, awsQueryError code MessageRejected.
Actual: HTTP 200 and a minted id. Replay detail: 'AssertionError: expected the call to be refused, but it succeeded and returned {\'MessageId\': \'0100e6a2ef83598c-25177fd9-a543-d9ba-36a7-25831489f1ac-000000\', \'ResponseMetadata\': {\'RequestId\': \'...\', \'HTTPStatusCode\': 200, \'HTTPHeaders\': {\'content-type\': \'text/xml\', ...}}}'. The MessageId, request id and date in that payload are per-run values and are not part of the requirement; the diverging facts are the 200 status and the absent MessageRejected error.
The same check's later steps (ConfigurationSetDoesNotExist for an unknown set, 'Configuration set <NAME> does not exist.', SentLast24Hours unchanged after a refusal, and the refused call not creating the set) were never reached because the control step above failed first.

## Steps to Reproduce
1. Boot the emulator and point a boto3 'ses' client at it (any region).
2. email = 'probe-1@example.com'; ses.verify_email_identity(EmailAddress=email)  — do NOT confirm it.
3. ses.create_configuration_set(ConfigurationSet={'Name': 'cfgset-real-1'}).
4. ses.send_raw_email(Source=email, Destinations=[email], RawMessage={'Data': b'From: probe-1@example.com\r\nTo: probe-1@example.com\r\nSubject: probe\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=UTF-8\r\n\r\nprobe body\r\n'}, ConfigurationSetName='cfgset-real-1').
5. Observe HTTP 200 with a MessageId. Expected: ClientError MessageRejected, 'Email address is not verified. The following identities failed the check in region <REGION>: probe-1@example.com'.
6. Re-replay ses.send-raw-email-unknown-configuration-set-fails.
