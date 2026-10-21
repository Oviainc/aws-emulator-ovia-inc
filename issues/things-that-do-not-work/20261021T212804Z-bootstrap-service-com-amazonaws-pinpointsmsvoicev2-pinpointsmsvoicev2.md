Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the Pinpoint SMS Voice V2 emulator

_Filed: 2026-10-21T21:28:04Z_
_Operation: service-com-amazonaws-pinpointsmsvoicev2-pinpointsmsvoicev2 | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 110 operations in this crate is an untouched scaffold: each `src/ops/*.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, and the service holds no state. Implement a FIRST, SMALL, stateful slice: the origination-identity + send-text happy path of AWS End User Messaging SMS (sms-voice), wire protocol `awsJson1_0`, `POST /` with `X-Amz-Target: PinpointSMSVoiceV2.<OperationName>`.

Happy path to implement, in order (fields per OPERATIONS.md):

1. **CreateConfigurationSet** — input `ConfigurationSetName` (required, len 1..64, `^[A-Za-z0-9_-]+$`), `Tags`, `ClientToken` (@idempotencyToken). Store a new configuration set; return `ConfigurationSetArn` (e.g. `arn:aws:sms-voice:<region>:<account>:configuration-set/<name>`), `ConfigurationSetName`, `Tags`, `CreatedTimestamp` read back from the stored record. A second create with the same name must fail `ConflictException` (HTTP 400).
2. **RequestPhoneNumber** — input `IsoCountryCode` (required), `MessageType` (PROMOTIONAL|TRANSACTIONAL, required), `NumberCapabilities` (required), `NumberType` (LONG_CODE|SIMULATOR|TEN_DLC|TOLL_FREE, required), plus optional `OptOutListName`, `PoolId`, `DeletionProtectionEnabled`, `Tags`, `ClientToken`. Mint and PERSIST a phone number record: generated `PhoneNumberId`, `PhoneNumberArn`, a synthetic E.164 `PhoneNumber` matching `^\+?[1-9][0-9]{1,18}$`, `Status` = ACTIVE, and echo-from-state `IsoCountryCode`, `MessageType`, `NumberCapabilities`, `NumberType`, `MonthlyLeasingPrice`, the `TwoWayEnabled`/`SelfManagedOptOutsEnabled`/`InternationalSendingEnabled`/`DeletionProtectionEnabled` defaults (false), `OptOutListName`, `Tags`, `CreatedTimestamp`.
3. **DescribePhoneNumbers** — input `PhoneNumberIds` (len ..5), `Filters`, `MaxResults` (1..100), `NextToken`. Must return the numbers created in step 2 as `PhoneNumbers` (list<PhoneNumberInformation>) built from the store; honour `PhoneNumberIds` selection (unknown id -> `ResourceNotFoundException`), apply `MaxResults` and emit/accept an opaque `NextToken` for the remainder. A released number must no longer appear.
4. **SendTextMessage** — input `DestinationPhoneNumber` (required), `OriginationIdentity` (PhoneNumber / PhoneNumberId / PhoneNumberArn / PoolId / SenderId), `MessageBody`, `MessageType`, `ConfigurationSetName`, `TimeToLive`, `Context`, `DryRun`. Resolve `OriginationIdentity` against the stored phone numbers (unknown -> `ResourceNotFoundException`) and `ConfigurationSetName` against stored configuration sets (unknown -> `ResourceNotFoundException`); generate a fresh unique `MessageId` and APPEND a message record to state (destination, body, origination id, timestamp, dry_run) so tests can assert the send happened. `DryRun=true` must still return a `MessageId` but must not be counted as a sent message.
5. **ReleasePhoneNumber** — input `PhoneNumberId` (required). Remove the number from state and return its last-known fields (`PhoneNumberArn`, `PhoneNumberId`, `PhoneNumber`, `Status`, `IsoCountryCode`, `MessageType`, `NumberCapabilities`, `NumberType`, `CreatedTimestamp`, ...). Unknown id -> `ResourceNotFoundException`; a number with `DeletionProtectionEnabled=true` -> `ConflictException`. After release, SendTextMessage using that origination identity must fail and DescribePhoneNumbers must not list it.
6. **DeleteConfigurationSet** — input `ConfigurationSetName` (required). Remove the set and return `ConfigurationSetArn`, `ConfigurationSetName`, `EventDestinations` (empty for the first slice), `DefaultMessageType`, `DefaultSenderId`, `CreatedTimestamp` from the deleted record. Unknown name -> `ResourceNotFoundException`; a second delete must also 400 `ResourceNotFoundException`.

Every one of these six MUST compute its response from the persisted store reached through `crate::state::service_state(&*self.engine)` — no canned bodies, no echoing the input without storing it, no random id without a record behind it. Delete the `SCAFFOLDED, NOT IMPLEMENTED` marker line in each implemented file. Honour `ClientToken` idempotency at least to the extent of not creating duplicates for a repeated token on CreateConfigurationSet / RequestPhoneNumber.

## State Model

In `src/state.rs`, on the service state struct:

- `configuration_sets: HashMap<String /*name*/, ConfigurationSet { name, arn, tags: Vec<Tag>, created_timestamp: SystemTime, default_message_type: Option<String>, default_sender_id: Option<String>, default_message_feedback_enabled: bool, event_destinations: Vec<EventDestination> }>`
- `phone_numbers: HashMap<String /*PhoneNumberId*/, PhoneNumberRecord { id, arn, phone_number: String /*E.164*/, status: String, iso_country_code, message_type, number_capabilities: Vec<String>, number_type, monthly_leasing_price: String, two_way_enabled: bool, two_way_channel_arn: Option<String>, self_managed_opt_outs_enabled: bool, opt_out_list_name: Option<String>, international_sending_enabled: bool, deletion_protection_enabled: bool, pool_id: Option<String>, registration_id: Option<String>, tags: Vec<Tag>, created_timestamp: SystemTime }>`
- `messages: Vec<SentMessage { message_id: String, destination_phone_number: String, origination_identity: String, message_body: Option<String>, message_type: Option<String>, configuration_set_name: Option<String>, context: HashMap<String,String>, dry_run: bool, sent_at: SystemTime }>`
- `client_tokens: HashMap<String /*ClientToken*/, String /*created resource id*/>` for idempotency
- a monotonic counter used to mint `PhoneNumberId` / synthetic E.164 numbers

Out of scope for this first slice (stay 501, filed separately later): pools, opt-out lists and keywords, sender ids, registrations and attachments, protect configurations, event destinations, RCS/notify/voice/media sending, spend limits, resource policies, tagging ops.

## Evidence

OPERATIONS.md at the crate root (`services/service-com-amazonaws-pinpointsmsvoicev2-pinpointsmsvoicev2/OPERATIONS.md`) lists 110 operations with the exact input/output members and errors cited above: CreateConfigurationSet (line 45), DeleteConfigurationSet (line 123), DescribeConfigurationSets (line 269), DescribePhoneNumbers (line 311), ReleasePhoneNumber (line 523), RequestPhoneNumber (line 535), SendTextMessage (line 577); the wire protocol block (lines 8-15) gives awsJson1_0 + `X-Amz-Target: PinpointSMSVoiceV2.<Op>`. Every file under `src/ops/` is an unmodified scaffold — e.g. `src/ops/send_text_message.rs:4` carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 221 returns `Err(crate::scaffolded::Scaffolded::not_implemented("SendTextMessage"))`, so the call answers HTTP 501 and persists nothing. No tests exercise any of these operations (only the generated `tests/state_roundtrip.rs`).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `pinpoint-sms-voice-v2` client at it.
2. CreateConfigurationSet(ConfigurationSetName="demo-set") -> expect 200 with ConfigurationSetName="demo-set", an ARN and a CreatedTimestamp; today returns 501 NotImplemented.
3. RequestPhoneNumber(IsoCountryCode="US", MessageType="TRANSACTIONAL", NumberCapabilities=["SMS"], NumberType="SIMULATOR") -> expect 200 with a PhoneNumberId, E.164 PhoneNumber and Status=ACTIVE.
4. DescribePhoneNumbers() -> expect the number from the previous step in PhoneNumbers with the same id/capabilities/country.
5. SendTextMessage(DestinationPhoneNumber="+14255550123", OriginationIdentity=<PhoneNumberId>, MessageBody="hello", ConfigurationSetName="demo-set") -> expect 200 with a unique MessageId; repeat and expect a DIFFERENT MessageId.
6. SendTextMessage with OriginationIdentity="pn-does-not-exist" -> expect 400 ResourceNotFoundException, not 501.
7. ReleasePhoneNumber(PhoneNumberId=<id>) -> expect 200 echoing the released number's stored fields; DescribePhoneNumbers() afterwards must not list it and a further SendTextMessage with that identity must fail ResourceNotFoundException.
8. DeleteConfigurationSet(ConfigurationSetName="demo-set") -> expect 200 with the stored ARN/CreatedTimestamp; calling it again must return 400 ResourceNotFoundException.
