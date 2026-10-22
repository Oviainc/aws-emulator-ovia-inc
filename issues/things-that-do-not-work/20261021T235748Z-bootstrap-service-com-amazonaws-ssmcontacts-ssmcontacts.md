Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: b135c4508c1fb26e9a8e4477f81bd51ec13345b2

# Bootstrap the SSM Contacts emulator

_Filed: 2026-10-21T23:57:48Z_
_Operation: service-com-amazonaws-ssmcontacts-ssmcontacts | Class: missing_

> _Note: this issue was identified against the code base at commit `b135c4508c1fb26e9a8e4477f81bd51ec13345b2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 39 operations in this crate are unmodified scaffolds answering HTTP 501 `NotImplemented` and the service holds zero state, so no client workflow works. Implement a FIRST, SMALL stateful slice covering the contact lifecycle — the service's primary resource — so a realistic client script runs end to end. Operations outside this slice stay stubbed and will be filed as individual gap issues later.

### Happy path (in order)
1. **CreateContact** — input `Alias` (required, len 1..255, `^[a-z0-9_\-]*$`), `Type` (required: `ESCALATION|ONCALL_SCHEDULE|PERSONAL`), `Plan` (required), optional `DisplayName`, `Tags`, `IdempotencyToken`. Must mint and PERSIST a real contact and return the generated `ContactArn` (required output, `arn:aws:ssm-contacts:<region>:<account>:contact/<alias>`). A second CreateContact with the same `Alias` must fail with `ConflictException` (409).
2. **CreateContactChannel** — input `ContactId` (ARN of an existing contact, required), `Name`, `Type` (`EMAIL|SMS|VOICE`), `DeliveryAddress` (required), optional `DeferActivation`. Must store the channel under the referenced contact and return the generated `ContactChannelArn`. An unknown `ContactId` must produce `ResourceNotFoundException` (404). New channels start with `ActivationStatus = NOT_ACTIVATED`.
3. **GetContact** — input `ContactId`; must return the stored `ContactArn`, `Alias`, `DisplayName`, `Type` and `Plan` exactly as created (not echoed from the request), and `ResourceNotFoundException` for an unknown ARN.
4. **ListContactChannels** — input `ContactId` (required), `MaxResults`, `NextToken`; must return the channels actually created for that contact in `ContactChannels` (each with `ContactArn`, `ContactChannelArn`, `Name`, `Type`, `DeliveryAddress`, `ActivationStatus`), honouring `MaxResults` and round-tripping `NextToken`.
5. **ListContacts** — input optional `AliasPrefix`, `Type`, `MaxResults`, `NextToken`; must return every persisted contact in `Contacts`, filtered by `AliasPrefix` and `Type` when given, with working pagination.
6. **DeleteContact** — input `ContactId`; must remove the contact from state so a subsequent GetContact/ListContacts no longer sees it (404 / absent). Deleting a contact that still has contact channels must fail with `ConflictException` (409), matching the real service; `DeleteContactChannel` on the channel first must then let the delete succeed.

Every one of these MUST compute its response from persisted state — no canned responses, no echoing the input back, no returning a random ARN that was never stored. A create followed by a read must return what was created, and a delete must be observable by the next read.

## State Model
Add to `src/state.rs` (`ComAmazonawsSsmcontactsSsmcontactsState`, reached via `crate::state::service_state(&*self.engine)`):

- `contacts: BTreeMap<String /* ContactArn */, Contact>` where `Contact { arn: String, alias: String, display_name: Option<String>, contact_type: ContactType /* ESCALATION|ONCALL_SCHEDULE|PERSONAL */, plan: Plan /* Stages(list of Stage{DurationInMinutes, Targets}) or RotationIds */, tags: Vec<Tag>, created_at: SystemTime }`
- `alias_index: BTreeMap<String /* alias */, String /* ContactArn */>` to enforce the 409 on duplicate aliases and to serve `AliasPrefix` filtering.
- `contact_channels: BTreeMap<String /* ContactChannelArn */, ContactChannel>` where `ContactChannel { arn: String, contact_arn: String, name: String, channel_type: ChannelType /* EMAIL|SMS|VOICE */, delivery_address: ContactChannelAddress, activation_status: ActivationStatus /* ACTIVATED|NOT_ACTIVATED */, activation_code: Option<String> }`, plus a per-contact ordering (insertion order) so ListContactChannels is stable.
- `idempotency_tokens: BTreeMap<String /* token */, String /* created ARN */>` so a repeated CreateContact/CreateContactChannel with the same `IdempotencyToken` returns the same ARN instead of creating a duplicate.

ARNs must be generated from the request's partition/region/account (per the model pattern `^arn:(aws|aws-cn|aws-us-gov):ssm-contacts:[-\w+=/,.@]*:[0-9]+:([\w+=/,.@:-]+)*$`) and stored as the resource key, so the ARN a create returns is the ARN every later call resolves.

Done = an integration test driving the six operations above over `awsJson1_1` (`POST /`, `X-Amz-Target: SSMContacts.<Op>`) passes with correct, stateful response bodies — not merely 2xx.

## Evidence

`services/service-com-amazonaws-ssmcontacts-ssmcontacts/OPERATIONS.md` lists all 39 operations with their input/output members and errors (CreateContact: required `Alias`/`Type`/`Plan` -> required `ContactArn`, errors incl. `ConflictException` 409; CreateContactChannel: required `ContactId`/`Name`/`Type`/`DeliveryAddress` -> required `ContactChannelArn`; GetContact returns `ContactArn`/`Alias`/`DisplayName`/`Type`/`Plan`; ListContacts and ListContactChannels are `@paginated(inputToken=NextToken, outputToken=NextToken, items=...)`; DeleteContact declares `ConflictException` and `ResourceNotFoundException`). README.md documents the wire protocol (awsJson1_1, target prefix `SSMContacts`) and the scaffold contract. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 39 handler files, including `create_contact.rs`, `create_contact_channel.rs`, `get_contact.rs`, `list_contacts.rs`, `list_contact_channels.rs` and `delete_contact.rs` — every one returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501). No tests exercise any behavior beyond `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `ssm-contacts` at it.
2. CreateContact with Alias="oncall-primary", Type="PERSONAL", Plan={Stages:[{DurationInMinutes:5,Targets:[]}]} — expect HTTP 200 with a generated ContactArn; today: HTTP 501 NotImplemented.
3. CreateContact again with the same Alias — expect ConflictException (409).
4. CreateContactChannel with ContactId=<ContactArn>, Name="email", Type="EMAIL", DeliveryAddress={SimpleAddress:"ops@example.com"} — expect 200 with a generated ContactChannelArn.
5. GetContact with ContactId=<ContactArn> — expect the Alias, Type and Plan exactly as created.
6. ListContactChannels with ContactId=<ContactArn> — expect exactly the one created channel with ActivationStatus=NOT_ACTIVATED.
7. ListContacts with AliasPrefix="oncall" — expect the created contact; with AliasPrefix="zzz" expect an empty list.
8. DeleteContact with ContactId=<ContactArn> while the channel exists — expect ConflictException (409); DeleteContactChannel, then DeleteContact — expect 200.
9. GetContact with the deleted ContactArn — expect ResourceNotFoundException (404); ListContacts no longer returns it.
