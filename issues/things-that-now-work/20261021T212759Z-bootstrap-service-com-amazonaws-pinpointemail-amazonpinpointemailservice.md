Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the Pinpoint Email emulator

_Filed: 2026-10-21T21:27:59Z_
_Operation: service-com-amazonaws-pinpointemail-amazonpinpointemailservice | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 42 operations in this crate is an unmodified scaffold: each `src/ops/*.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Scaffolded::not_implemented(..)` (HTTP 501), and the crate holds no state at all. Implement a FIRST, SMALL, stateful slice: the email-identity lifecycle plus a send, wired through one shared in-memory store so a realistic client script works end to end.

Wire protocol: `restJson1`, sigv4 signing name `ses`, endpoint prefix `email` (see OPERATIONS.md "Wire protocol").

Happy path to implement, in order:

1. **CreateEmailIdentity** — `POST /v1/email/identities`. Input: `EmailIdentity` (required), `Tags`. Must insert a new identity record into the store keyed by `EmailIdentity`. Classify `IdentityType` from the value: contains `@` -> `EMAIL_ADDRESS`, otherwise `DOMAIN`. Return `IdentityType`, `VerifiedForSendingStatus` (false for a fresh identity, as real AWS does until verification), and for a DOMAIN identity a `DkimAttributes` with `SigningEnabled=true`, `Status=PENDING` and generated `Tokens`. Creating the same identity twice must not create a duplicate entry (the second call updates/keeps the single record, and `ListEmailIdentities` must still show it once).
2. **GetEmailIdentity** — `GET /v1/email/identities/{EmailIdentity}`. Must look the identity up in the store and return exactly what CreateEmailIdentity persisted: `IdentityType`, `VerifiedForSendingStatus`, `FeedbackForwardingStatus` (default true in real AWS; persist whatever the record holds), `DkimAttributes`, `MailFromAttributes`, `Tags`. Unknown identity -> `NotFoundException` (HTTP 404), not 200 and not 501.
3. **ListEmailIdentities** — `GET /v1/email/identities`. Input: `NextToken`, `PageSize` (both `@httpQuery`). Must return one `IdentityInfo` per stored identity (`IdentityType`, `IdentityName`, `SendingEnabled`), honour `PageSize` by truncating the page, and return an opaque `NextToken` when more identities remain — a token that, fed back in, returns the following page and eventually no token. No canned/empty list.
4. **CreateConfigurationSet** — `POST /v1/email/configuration-sets`. Input: `ConfigurationSetName` (required), `TrackingOptions`, `DeliveryOptions`, `ReputationOptions`, `SendingOptions`, `Tags`. Empty output body, but the set must be persisted so that **GetConfigurationSet** (`GET /v1/email/configuration-sets/{ConfigurationSetName}`) echoes back the stored name and all four option structures and tags, and a duplicate name returns `AlreadyExistsException` (HTTP 400).
5. **SendEmail** — `POST /v1/email/outbound-emails`. Input: `FromEmailAddress`, `Destination` (required), `Content` (required), `ReplyToAddresses`, `FeedbackForwardingEmailAddress`, `EmailTags`, `ConfigurationSetName`. Must validate against persisted state rather than always succeeding: a `ConfigurationSetName` that was never created -> `NotFoundException` (404); a `Destination` with no `ToAddresses`/`CcAddresses`/`BccAddresses` or an empty `Content` -> `BadRequestException` (400). On success, generate a fresh unique `MessageId`, append the sent message (from, destination, subject/body, configuration set, tags, timestamp) to the store's sent-message log, and return that `MessageId`. Two sends must return two different MessageIds.
6. **DeleteEmailIdentity** — `DELETE /v1/email/identities/{EmailIdentity}`. Must remove the record from the store; unknown identity -> `NotFoundException` (404). After deletion, GetEmailIdentity on it must 404 and ListEmailIdentities must no longer include it.

NEVER STUBS: each of these must compute its response from the persisted store. Hardcoded outputs, echoing the request back without storing it, or minting an ID with no record behind it are failures of this issue. Errors must be the modeled restJson1 error shapes with the modeled HTTP statuses (`NotFoundException` 404, `AlreadyExistsException`/`BadRequestException` 400).

## State Model

One process-wide store behind a mutex, partitioned per account/region:

- `identities: Map<String /* EmailIdentity */, EmailIdentity { identity_name: String, identity_type: IdentityType (DOMAIN|EMAIL_ADDRESS|MANAGED_DOMAIN), verified_for_sending: bool, feedback_forwarding_status: bool, sending_enabled: bool, dkim: DkimAttributes { signing_enabled: bool, status: DkimStatus, tokens: Vec<String> }, mail_from: Option<MailFromAttributes { mail_from_domain, mail_from_domain_status, behavior_on_mx_failure }>, tags: Vec<Tag>, created_at: Timestamp }>`
- `configuration_sets: Map<String /* ConfigurationSetName */, ConfigurationSet { name: String, tracking_options: Option<TrackingOptions>, delivery_options: Option<DeliveryOptions>, reputation_options: Option<ReputationOptions>, sending_options: Option<SendingOptions>, tags: Vec<Tag>, event_destinations: Map<String, EventDestination> }>`
- `sent_messages: Vec<SentMessage { message_id: String, from_email_address: Option<String>, destination: Destination, content_subject: Option<String>, content_body: Option<String>, reply_to_addresses: Vec<String>, email_tags: Vec<MessageTag>, configuration_set_name: Option<String>, sent_at: Timestamp }>` (backs MessageId uniqueness and future GetAccount/send-quota accounting)

Iteration order for list operations must be deterministic (e.g. a BTreeMap or insertion-ordered index) so pagination tokens are stable across calls.

Everything else in the service — dedicated IP pools and warmup, deliverability dashboard/test reports, domain statistics, blacklist reports, event destinations, Put* attribute setters, Tag/Untag/ListTagsForResource, GetAccount — stays 501 for now and will be filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-pinpointemail-amazonpinpointemailservice/OPERATIONS.md` lists all 42 operations with their routes, input/output members and errors — CreateEmailIdentity (line 54), DeleteEmailIdentity (82), GetConfigurationSet (103), GetEmailIdentity (160), ListConfigurationSets (167), ListEmailIdentities (199), SendEmail (298) — and the "Wire protocol" section gives restJson1 / signing name `ses`. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 42 handler files in this crate (create_email_identity.rs, get_email_identity.rs, list_email_identities.rs, create_configuration_set.rs, get_configuration_set.rs, send_email.rs, delete_email_identity.rs included), each returning `crate::scaffolded::Scaffolded::not_implemented(..)` => HTTP 501, so no state exists and no test asserts any behavior.

## Steps to Reproduce

1. Point an AWS SDK/CLI `pinpoint-email` client at the emulator endpoint (signing name `ses`).
2. CreateEmailIdentity with EmailIdentity="example.com" -> expect 200 with IdentityType=DOMAIN, VerifiedForSendingStatus=false, DkimAttributes present; actual: HTTP 501 NotImplemented.
3. GetEmailIdentity for "example.com" -> expect 200 echoing the persisted identity type/verification/DKIM; actual: 501.
4. ListEmailIdentities with PageSize=1 after creating two identities -> expect one IdentityInfo plus a NextToken, and the second page returning the other identity; actual: 501.
5. CreateConfigurationSet with ConfigurationSetName="bootstrap-set" and SendingOptions{SendingEnabled:true}, then GetConfigurationSet("bootstrap-set") -> expect the stored name and options returned; repeating CreateConfigurationSet -> expect AlreadyExistsException (400); actual: 501.
6. SendEmail with FromEmailAddress="sender@example.com", Destination.ToAddresses=["dest@example.org"], Content.Simple.Subject/Body set, ConfigurationSetName="bootstrap-set" -> expect 200 with a unique MessageId, and a second send returning a different MessageId; actual: 501.
7. SendEmail with ConfigurationSetName="no-such-set" -> expect NotFoundException (404); SendEmail with an empty Destination -> expect BadRequestException (400); actual: 501.
8. DeleteEmailIdentity("example.com") -> expect 200, then GetEmailIdentity("example.com") -> expect NotFoundException (404) and ListEmailIdentities no longer listing it; actual: 501 for all.
