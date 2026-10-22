Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the sesv2 (Simple Email Service v2) emulator

_Filed: 2026-10-21T21:28:14Z_
_Operation: service-com-amazonaws-sesv2-simpleemailservice-v2 | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in this crate is an unmodified scaffold: each `src/ops/*.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented`, and `src/state.rs` holds no service data. Implement a FIRST, SMALL happy-path slice — the email-identity lifecycle plus a send — statefully, so a realistic client script works end to end. Everything else stays 501 and is filed separately later.

## Happy path (implement these 5 operations, in this order)

1. **CreateEmailIdentity** — `POST /v2/email/identities`. Input: `EmailIdentity` (required, len 1..), `Tags`, `DkimSigningAttributes`, `ConfigurationSetName`. Must persist a new identity record keyed by `EmailIdentity` and return the computed `IdentityType` (`EMAIL_ADDRESS` when the value contains `@`, otherwise `DOMAIN`), `VerifiedForSendingStatus`, and `DkimAttributes` consistent with what was stored (e.g. `SigningEnabled`, `Status`, `SigningAttributesOrigin`, and `Tokens` for a domain identity). A second create of the same identity must fail with `AlreadyExistsException` (HTTP 400).
2. **GetEmailIdentity** — `GET /v2/email/identities/{EmailIdentity}`. Must read the persisted record and return `IdentityType`, `VerifiedForSendingStatus`, `FeedbackForwardingStatus`, `DkimAttributes`, `Tags`, `ConfigurationSetName`, `VerificationStatus` exactly as created (tags and configuration set must round-trip, not be defaulted). Unknown identity -> `NotFoundException` (HTTP 404).
3. **ListEmailIdentities** — `POST /v2/email/list-identities`. Must enumerate the stored identities as `EmailIdentities` (`IdentityInfo{IdentityType, IdentityName, SendingEnabled, VerificationStatus}`) — the list must grow on create and shrink on delete. Honour `PageSize` by truncating and returning an opaque `NextToken` that resumes where the previous page stopped (operation is `@paginated(inputToken=NextToken, outputToken=NextToken)`).
4. **SendEmail** — `POST /v2/email/outbound-emails`. Input: `Content` (required), `FromEmailAddress`, `Destination`, `ConfigurationSetName`, `EmailTags`, `ReplyToAddresses`. Must generate a fresh unique `MessageId` per call, append the message (from, destinations, subject/body, tags, timestamp) to a persisted sent-message log, and bump per-account send counters. Sending from an identity that was never created must fail with `MessageRejected` or `NotFoundException` per the model, and a missing `Content` must fail `BadRequestException` (HTTP 400) rather than succeed.
5. **DeleteEmailIdentity** — `DELETE /v2/email/identities/{EmailIdentity}`. Must remove the record so a subsequent GetEmailIdentity returns `NotFoundException` (404) and the identity disappears from ListEmailIdentities. Deleting an unknown identity -> `NotFoundException` (404).

No canned, echoed or hardcoded responses: every field in every response above must be computed from state persisted in the shared engine (reached via `crate::state::service_state(&*self.engine)`), and a create followed by a read must return what was created. A random `MessageId` without storing the message is a failure.

## State Model

In `ComAmazonawsSesv2SimpleemailserviceV2State`:

- `identities: BTreeMap<String /*EmailIdentity*/, EmailIdentity>` where
  `EmailIdentity { name: String, identity_type: IdentityType, verified_for_sending: bool, feedback_forwarding: bool, verification_status: VerificationStatus, dkim: DkimState{ signing_enabled: bool, status: DkimStatus, tokens: Vec<String>, signing_attributes_origin: DkimSigningAttributesOrigin }, configuration_set_name: Option<String>, tags: Vec<Tag>, policies: BTreeMap<String,String>, created_at: SystemTime }`
- `sent_messages: Vec<SentMessage>` where
  `SentMessage { message_id: String, from: Option<String>, to: Vec<String>, cc: Vec<String>, bcc: Vec<String>, subject: Option<String>, body: Option<String>, configuration_set_name: Option<String>, email_tags: Vec<MessageTag>, sent_at: SystemTime }` (ordered oldest-first; the log is what lets a test assert the sent body)
- `send_counters: SendQuotaState { sent_last_24h: f64, max_24h: f64, max_send_rate: f64 }` incremented by SendEmail
- `list_tokens`: pagination cursor encoding for ListEmailIdentities (an opaque token carrying the last returned identity name is sufficient).

## Done when

An integration test performs create -> get -> list -> send -> delete -> get(404) and every assertion on the response BODIES (not just status) holds.

## Evidence

services/service-com-amazonaws-sesv2-simpleemailservice-v2/OPERATIONS.md documents the five operations with the exact routes, inputs, outputs and errors cited above: CreateEmailIdentity (line 96, `POST /v2/email/identities`, AlreadyExistsException), GetEmailIdentity (line 356, `GET /v2/email/identities/{EmailIdentity}`, NotFoundException 404), ListEmailIdentities (line 482, `POST /v2/email/list-identities`, @paginated NextToken/PageSize), SendEmail (line 769, `POST /v2/email/outbound-emails`, output MessageId, MessageRejected/BadRequestException), DeleteEmailIdentity (line 194, `DELETE /v2/email/identities/{EmailIdentity}`, NotFoundException 404). src/ops/send_email.rs line 4 carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and returns `Scaffolded::not_implemented(..)` (HTTP 501), which README.md lines 36-53 confirm is the state of every generated op file in this crate.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK sesv2 client at it.
2. CreateEmailIdentity(EmailIdentity='sender@example.com', Tags=[{Key='env',Value='test'}]) -> expect 200 with IdentityType=EMAIL_ADDRESS; today 501 NotImplemented.
3. CreateEmailIdentity with the same address again -> expect AlreadyExistsException (HTTP 400).
4. GetEmailIdentity(EmailIdentity='sender@example.com') -> expect 200 echoing the stored IdentityType, VerificationStatus, DkimAttributes and Tags=[env=test].
5. ListEmailIdentities() -> expect EmailIdentities to contain IdentityInfo for 'sender@example.com'; repeat with PageSize=1 after creating a second identity and follow NextToken to get the second page.
6. SendEmail(FromEmailAddress='sender@example.com', Destination={ToAddresses=['rcpt@example.net']}, Content={Simple={Subject={Data='hi'},Body={Text={Data='hello'}}}}) -> expect 200 with a unique MessageId, and the message recorded in state.
7. SendEmail with no Content -> expect BadRequestException (HTTP 400), not success.
8. DeleteEmailIdentity(EmailIdentity='sender@example.com') -> expect 200.
9. GetEmailIdentity(EmailIdentity='sender@example.com') -> expect NotFoundException (HTTP 404) and the identity absent from ListEmailIdentities.
