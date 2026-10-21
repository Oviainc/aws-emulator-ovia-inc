Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the SES (Simple Email Service) emulator

_Filed: 2026-10-21T21:28:04Z_
_Operation: service-com-amazonaws-ses-simpleemailservice | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 71 operations in `services/service-com-amazonaws-ses-simpleemailservice/src/ops/` is an unmodified scaffold: each file carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, and the service holds no state. The first slice should make SES's core identity-verification + send loop work end-to-end over the `awsQuery` protocol (sigv4 name `ses`, endpoint prefix `email`).

Implement these SIX operations statefully, in this order:

1. **VerifyEmailIdentity** — input `EmailAddress` (string, required); output has no members. Must INSERT the address into the identity store as an email-address identity. SES sandbox behaviour: the real service records the identity immediately with `VerificationStatus=Pending`; for the emulator record it (Pending is acceptable, or auto-Success if the emulator chooses, but the choice must be consistent and observable by the next two reads). Idempotent re-verification of the same address must not duplicate it.
2. **ListIdentities** — input `IdentityType` (enum `Domain|EmailAddress`), `NextToken`, `MaxItems`; output `Identities` (list<string>, required) + `NextToken`. Must return the identities actually stored by VerifyEmailIdentity/VerifyDomainIdentity, filtered by `IdentityType` when given, in a stable order, honouring `MaxItems` and round-tripping `NextToken` (op is `@paginated(inputToken=NextToken, outputToken=NextToken, items=Identities)`). Never a canned list.
3. **GetIdentityVerificationAttributes** — input `Identities` (list<string>, required); output `VerificationAttributes` (map<string,IdentityVerificationAttributes>, required). For each requested identity that exists, return `IdentityVerificationAttributes{VerificationStatus (required enum: Pending|Success|Failed|TemporaryFailure), VerificationToken}`; `VerificationToken` is set for domain identities and null for email-address identities (per the model's own doc). Identities that were never verified must be ABSENT from the map (not returned with a fabricated status).
4. **SendEmail** — input `Source` (required), `Destination` (required), `Message` (required), plus `ReplyToAddresses`, `ReturnPath`, `ConfigurationSetName`, `Tags`; output `MessageId` (string, required). Must generate a fresh unique `MessageId` per call, append the message to a per-account sent-message log (recording source, destination, subject/body, timestamp), and increment the send counters. Reject a `Source` that is not a verified identity with the modeled `MessageRejected` (HTTP 400, message in the shape of "Email address is not verified"), and reject an unknown `ConfigurationSetName` with `ConfigurationSetDoesNotExistException` (HTTP 400). No echoed/hardcoded MessageId.
5. **GetSendQuota** — no input; output `Max24HourSend`, `MaxSendRate`, `SentLast24Hours` (all double, required). `SentLast24Hours` must be COMPUTED from the sent-message log written by SendEmail (count of messages in the trailing 24h), not a constant 0. `Max24HourSend`/`MaxSendRate` come from the account record (sandbox-like defaults are fine, e.g. 200 / 1.0), stored in state so later work can change them.
6. **DeleteIdentity** — input `Identity` (string, required); output has no members. Must REMOVE the identity from the store so a following ListIdentities omits it and GetIdentityVerificationAttributes no longer reports it; deleting an unknown identity succeeds (SES returns 200).

Every one of these must compute its response from persisted state reached via `crate::state::service_state(&*self.engine)` — no canned responses, no echoing the input back, no random identifier that is not stored. A VerifyEmailIdentity followed by ListIdentities / GetIdentityVerificationAttributes must show exactly what was verified, and a SendEmail must be visible in GetSendQuota's `SentLast24Hours`. Delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line in each of the six handlers as they are implemented.

All other operations (templates, receipt rules/filters, configuration sets, identity policies, DKIM/notification attributes, SendRawEmail/SendTemplatedEmail/SendBulkTemplatedEmail, etc.) stay at 501 for this slice and will be filed as individual gap issues later.

## State Model

In `src/state.rs` (`ComAmazonawsSesSimpleemailserviceState`, lock-guarded):

- `identities: BTreeMap<String, Identity>` keyed by the identity string (email address or domain), where
  `Identity { name: String, identity_type: IdentityType /* EmailAddress | Domain */, verification_status: VerificationStatus /* Pending|Success|Failed|TemporaryFailure */, verification_token: Option<String> /* Some(..) for Domain, None for EmailAddress */, dkim_tokens: Vec<String>, verified_at: SystemTime }`
- `sent_messages: Vec<SentMessage>` as the append-only send log, where
  `SentMessage { message_id: String, source: String, destinations: Vec<String> /* To+Cc+Bcc flattened */, subject: Option<String>, body: Option<String>, configuration_set: Option<String>, tags: Vec<(String,String)>, sent_at: SystemTime }`
- `account: AccountSettings { max_24_hour_send: f64 /* default 200.0 */, max_send_rate: f64 /* default 1.0 */, sending_enabled: bool /* default true */ }` — `SentLast24Hours` is derived by counting `sent_messages` with `sent_at` within the last 24h, never stored as a mutable counter that can drift.
- `configuration_sets: BTreeMap<String, ConfigurationSet>` — needed only so SendEmail can validate `ConfigurationSetName`; may start empty.

IDs: `message_id` should be a fresh opaque token per SendEmail (SES-shaped, e.g. `0100<hex>-<hex>-000000`), stored on the `SentMessage` it identifies.

## Done when

A client script running VerifyEmailIdentity → ListIdentities → GetIdentityVerificationAttributes → SendEmail → GetSendQuota → DeleteIdentity → ListIdentities succeeds with correct, stateful bodies at every step (not merely 2xx), and SendEmail from an unverified Source returns `MessageRejected` (HTTP 400).

## Evidence

`services/service-com-amazonaws-ses-simpleemailservice/OPERATIONS.md` (generated from `smithy-model/com-amazonaws-ses-simpleemailservice.json`): protocol `awsQuery`, SDK id `SES`, sigv4 `ses`, endpoint prefix `email`; VerifyEmailIdentity input `EmailAddress` (required), no output members; ListIdentities `@paginated(inputToken=NextToken, outputToken=NextToken, items=Identities)` with input `IdentityType|NextToken|MaxItems` and required output `Identities`; GetIdentityVerificationAttributes input `Identities` (required) → required `VerificationAttributes` map<string,IdentityVerificationAttributes>; SendEmail input `Source`/`Destination`/`Message` required → required `MessageId`, errors `MessageRejected`, `ConfigurationSetDoesNotExistException`, `AccountSendingPausedException`, `ConfigurationSetSendingPausedException`, `MailFromDomainNotVerifiedException` (all HTTP 400); GetSendQuota no input → required `Max24HourSend`/`MaxSendRate`/`SentLast24Hours`; DeleteIdentity input `Identity` (required), no output, no errors. `smithy_query` on `IdentityVerificationAttributes` confirms required `VerificationStatus` and a `VerificationToken` documented as "Null for email address identities". All 71 handlers under `src/ops/` match `^//! SCAFFOLDED, NOT IMPLEMENTED:` (grep over the ops directory) and return `Scaffolded::not_implemented`, so there is no state and no behaviour today; no tests assert any of the above (only the generated `tests/state_roundtrip.rs` example exists).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI SES v1 client at it (`aws ses ... --endpoint-url <emulator>`).
2. `aws ses verify-email-identity --email-address sender@example.com` — currently HTTP 501 NotImplemented; must succeed and record the identity.
3. `aws ses list-identities --identity-type EmailAddress` — must return exactly `["sender@example.com"]` from state, honouring --max-items/--starting-token.
4. `aws ses get-identity-verification-attributes --identities sender@example.com` — must return a VerificationAttributes entry for that address with a VerificationStatus and no VerificationToken; an unverified address must be absent from the map.
5. `aws ses send-email --from sender@example.com --destination ToAddresses=rcpt@example.com --message 'Subject={Data=hi},Body={Text={Data=hello}}'` — must return a fresh unique MessageId and log the message.
6. `aws ses get-send-quota` — SentLast24Hours must now be 1 (computed from the send log), not 0.
7. `aws ses send-email --from unverified@example.com ...` — must fail with MessageRejected (HTTP 400), not succeed.
8. `aws ses delete-identity --identity sender@example.com` then `aws ses list-identities` — the identity must be gone.
