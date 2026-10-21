Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 16fab4a727e8e34e48fe5cd80cb0080f7932bd9f

# Bootstrap the socialmessaging emulator

_Filed: 2026-10-21T23:18:23Z_
_Operation: service-com-amazonaws-socialmessaging-socialmessaging | Class: stub_

> _Note: this issue was identified against the code base at commit `16fab4a727e8e34e48fe5cd80cb0080f7932bd9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 38 operations in `services/service-com-amazonaws-socialmessaging-socialmessaging/src/ops/` is an untouched scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. Implement a FIRST SLICE: the WhatsApp Business Account (WABA) link -> list -> read -> send -> unlink lifecycle, statefully, over the `restJson1` wire protocol (sigv4 name `social-messaging`).

### Happy path to implement (in order)

1. **AssociateWhatsAppBusinessAccount** — `POST /v1/whatsapp/signup`. Input `signupCallback` (WhatsAppSignupCallback{accessToken required, callbackUrl}) and/or `setupFinalization` (WhatsAppSetupFinalization{associateInProgressToken required, phoneNumbers required, phoneNumberParent, waba}). Must MINT and PERSIST a new linked account: generate `id` matching `^waba-.*$` (e.g. `waba-` + 32 hex chars), an `arn` matching `^arn:.*:waba/[0-9a-zA-Z]+$`, a Meta-style `wabaId`, `wabaName` from the `waba` setup block, `linkDate` = now, `registrationStatus` = COMPLETE, and one persisted phone-number record per entry in `setupFinalization.phoneNumbers` (each with its own `phone-number-id-...` id + ARN). Return `linkedWhatsAppBusinessAccountId` = the id actually stored, `statusCode` 200, and `signupCallbackResult` when `signupCallback` was supplied. When only `signupCallback` is given, store the in-progress token so a later `setupFinalization` call with that `associateInProgressToken` resolves to the same pending signup. Reject a request supplying both `waba` and `phoneNumberParent` with `InvalidParametersException` (400), and an unknown `phoneNumberParent` with `ResourceNotFoundException` (404).
2. **ListLinkedWhatsAppBusinessAccounts** — `GET /v1/whatsapp/waba/list`. Must return one `LinkedWhatsAppBusinessAccountSummary` per account actually in state (`arn`, `id`, `wabaId`, `registrationStatus`, `linkDate`, `wabaName`, `eventDestinations`, optional `datasetId`), honouring `maxResults` (1..100) and round-tripping `nextToken` (len 1..600) as a real cursor — never a canned empty list and never a token that is ignored.
3. **GetLinkedWhatsAppBusinessAccount** — `GET /v1/whatsapp/waba/details?id=`. Must look the account up by `id` (accept both the `waba-...` form and the account ARN) and return `account` (LinkedWhatsAppBusinessAccount) rebuilt from stored fields, including `phoneNumbers` (WhatsAppPhoneNumberSummaryList) for the phone numbers registered at association time and `eventDestinations` as last written. Unknown id -> `ResourceNotFoundException` (404), not 200 and not 501.
4. **GetLinkedWhatsAppBusinessAccountPhoneNumber** — `GET /v1/whatsapp/waba/phone/details?id=`. Must resolve a stored `phone-number-id-...` to `phoneNumber` (WhatsAppPhoneNumberDetail: `arn`, `phoneNumber`, `phoneNumberId`, `metaPhoneNumberId`, `displayPhoneNumberName`, `displayPhoneNumber`, `qualityRating`) plus `linkedWhatsAppBusinessAccountId` = the owning WABA id. Unknown id -> `ResourceNotFoundException` (404).
5. **SendWhatsAppMessage** — `POST /v1/whatsapp/send`. Input `originationPhoneNumberId` (required), `message` (blob 1..2048000, required), `metaApiVersion` (required). Must VALIDATE that `originationPhoneNumberId` names a phone number registered by step 1 (otherwise `ResourceNotFoundException` 404), record the sent message (origination id, message bytes, timestamp) in state, and return a freshly minted `messageId` that is unique per call — not a constant and not an echo of the input.
6. **DisassociateWhatsAppBusinessAccount** — `DELETE /v1/whatsapp/waba/disassociate?id=`. Must REMOVE the account and its phone numbers from state and return an empty 200 body. `@idempotent`: a second call for the same id, and any id never linked, surfaces `ResourceNotFoundException` (404) per the model's error list; after a successful call the account must be gone from both `GetLinkedWhatsAppBusinessAccount` and `ListLinkedWhatsAppBusinessAccounts`.

Also wire **PutWhatsAppBusinessAccountEventDestinations** (`PUT /v1/whatsapp/waba/eventdestinations`) if cheap: it writes `eventDestinations` (max 1) onto the stored account, and `GetLinkedWhatsAppBusinessAccount` must then return exactly what was written — it is the quickest proof the store is real.

### Rules

- NEVER STUBS: every one of these must compute its response from persisted state. Hardcoded ids, echoed inputs, and random-id-without-storage are failures — a generated `waba-...` id that a following `GetLinkedWhatsAppBusinessAccount` cannot resolve is a bug, not an implementation.
- State is shared through the engine on `self` (`crate::state::service_state(&*self.engine)`), never a fresh `InMemoryEngine`, so writes survive across requests.
- Errors use the modelled shapes/statuses from the model: `InvalidParametersException` 400, `ResourceNotFoundException` 404, `AccessDeniedByMetaException` 403, `ThrottledRequestException` 429, `DependencyException` 502, `InternalServiceException` 500.
- Done = an integration test that runs steps 1-6 in sequence against one running emulator and asserts the response BODIES (the listed account appears with the wabaName that was associated, the detail call returns the phone numbers that were set up, the message id is new each call, the account is absent after disassociate) — not merely that each call returned 2xx.

## State Model

In `src/state.rs` (`ComAmazonawsSocialmessagingSocialmessagingState`):

- `linked_accounts: HashMap<String /* waba id, e.g. waba-<32hex> */, LinkedAccount>` where
  `LinkedAccount { id: String, arn: String, waba_id: String, waba_name: String, registration_status: RegistrationStatus, link_date: SystemTime, event_destinations: Vec<WhatsAppBusinessAccountEventDestination>, phone_number_ids: Vec<String>, dataset_id: Option<String>, marketing_messages_onboarding_status: Option<String>, tags: HashMap<String,String> }`
- `phone_numbers: HashMap<String /* phone-number-id-<32hex> */, PhoneNumberRecord>` where
  `PhoneNumberRecord { phone_number_id: String, arn: String, waba_id: String /* owning linked account id */, phone_number: String, meta_phone_number_id: String, display_phone_number: String, display_phone_number_name: String, quality_rating: String, data_localization_region: Option<String>, call_settings: Option<WhatsAppCallSettings> }`
- `pending_signups: HashMap<String /* associateInProgressToken */, PendingSignup { access_token: String, callback_url: Option<String>, created_at: SystemTime }>`
- `sent_messages: Vec<SentMessage { message_id: String, origination_phone_number_id: String, message: Vec<u8>, meta_api_version: String, sent_at: SystemTime }>`
- `arn_index: HashMap<String /* arn */, String /* resource id */>` so ARN-form `id` inputs resolve to the same record as the short form.
- `id_seq: u64` for deterministic-but-unique id/message-id minting.

Operations outside this slice (flows, message templates, media, datasets, public keys, call events, tagging) stay 501 and are filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-socialmessaging-socialmessaging/OPERATIONS.md` lists all 38 operations with routes/members/errors, and `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-socialmessaging-socialmessaging/src/ops/` matches all 38 handler files — every operation returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` => HTTP 501, with zero state. Field names/constraints above come from OPERATIONS.md plus the Smithy model shapes `LinkedWhatsAppBusinessAccount`, `LinkedWhatsAppBusinessAccountSummary`, `WhatsAppSignupCallback`, `WhatsAppSetupFinalization` and `WhatsAppPhoneNumberDetail` (required members: account `arn/id/wabaId/registrationStatus/linkDate/wabaName/eventDestinations/phoneNumbers`; phone `arn/phoneNumber/phoneNumberId/metaPhoneNumberId/displayPhoneNumberName/displayPhoneNumber/qualityRating`). The id/ARN patterns `(^waba-.*$)|(^arn:.*:waba/[0-9a-zA-Z]+$)` and `(^phone-number-id-.*$)|(^arn:.*:phone-number-id/[0-9a-zA-Z]+$)` are model-enforced. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `socialmessaging` client at it (restJson1, signing name `social-messaging`).
2. AssociateWhatsAppBusinessAccount with a setupFinalization containing a waba block and one phone number -> expect 200 and a `linkedWhatsAppBusinessAccountId` of the form `waba-...`. Today: HTTP 501 NotImplemented.
3. ListLinkedWhatsAppBusinessAccounts -> expect exactly one summary whose `id` equals the id returned above and whose `wabaName` equals the name supplied.
4. GetLinkedWhatsAppBusinessAccount(id=<that id>) -> expect the account with `phoneNumbers` containing the phone number set up in step 2, plus `linkDate` and `registrationStatus`.
5. GetLinkedWhatsAppBusinessAccountPhoneNumber(id=<the phone-number-id from step 4>) -> expect its detail and `linkedWhatsAppBusinessAccountId` = the WABA id.
6. SendWhatsAppMessage(originationPhoneNumberId=<that phone-number-id>, message=<bytes>, metaApiVersion="v20.0") -> expect a unique `messageId`; repeat and expect a DIFFERENT messageId.
7. SendWhatsAppMessage with originationPhoneNumberId=`phone-number-id-00000000000000000000000000000000` -> expect ResourceNotFoundException (404).
8. DisassociateWhatsAppBusinessAccount(id=<the WABA id>) -> expect 200; then GetLinkedWhatsAppBusinessAccount(same id) -> ResourceNotFoundException (404) and ListLinkedWhatsAppBusinessAccounts -> empty list.
