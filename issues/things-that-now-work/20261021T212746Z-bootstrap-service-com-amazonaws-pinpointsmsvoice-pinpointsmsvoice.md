Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the Pinpoint SMS Voice emulator

_Filed: 2026-10-21T21:27:46Z_
_Operation: service-com-amazonaws-pinpointsmsvoice-pinpointsmsvoice | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 8 operations in `src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state. Implement a first functional slice covering the configuration-set lifecycle plus a voice send, so a realistic client script works end-to-end.

### Happy path (implement these 6, in this order)
1. **CreateConfigurationSet** — `POST /v1/sms-voice/configuration-sets`, input `ConfigurationSetName` (JSON body), empty 200 body. Must INSERT the named set into state; a second create of the same name must fail with `AlreadyExistsException` (HTTP 409); a missing/empty name is `BadRequestException` (HTTP 400).
2. **ListConfigurationSets** — `GET /v1/sms-voice/configuration-sets`, query params `NextToken`, `PageSize` (both strings), output `ConfigurationSets` (list of names) + `NextToken`. Must return the names actually stored by step 1 (not a canned list), honour `PageSize` as the page bound and return an opaque `NextToken` that resumes the listing; omit `NextToken` on the last page.
3. **CreateConfigurationSetEventDestination** — `POST /v1/sms-voice/configuration-sets/{ConfigurationSetName}/event-destinations`, inputs `ConfigurationSetName` (@httpLabel, required), `EventDestinationName`, `EventDestination` (`EventDestinationDefinition`: `Enabled`, `MatchingEventTypes`, `CloudWatchLogsDestination`, `KinesisFirehoseDestination`, `SnsDestination`). Empty 200 body. Must store the destination under the set, keyed by `EventDestinationName`, persisting ALL of the above sub-fields. Unknown set -> `NotFoundException` (404); duplicate destination name -> `AlreadyExistsException` (409).
4. **GetConfigurationSetEventDestinations** — `GET /v1/sms-voice/configuration-sets/{ConfigurationSetName}/event-destinations`, output `EventDestinations` (list of `EventDestination`). Must reconstruct each stored destination with `Name` set to the name used at create time and every destination sub-structure round-tripped byte-for-byte with what step 3 sent. Unknown set -> `NotFoundException` (404). Empty list (not 404) for a set with no destinations.
5. **SendVoiceMessage** — `POST /v1/sms-voice/voice/message`, inputs `CallerId`, `ConfigurationSetName`, `DestinationPhoneNumber`, `OriginationPhoneNumber`, `Content` (`VoiceMessageContent`: `PlainTextMessage` | `SSMLMessage` | `CallInstructionsMessage`), output `MessageId`. Must generate a fresh unique `MessageId` per call AND record the sent message in state (destination, origination, caller id, content, configuration set, timestamp) so later assertions/debug can observe it. A different `MessageId` on every call, never a constant.
6. **DeleteConfigurationSet** — `DELETE /v1/sms-voice/configuration-sets/{ConfigurationSetName}`, empty 200 body. Must REMOVE the set and its event destinations from state; a second delete, or a subsequent GetConfigurationSetEventDestinations on it, must return `NotFoundException` (404), and ListConfigurationSets must no longer show the name.

NEVER STUBS: each of the six must compute its response from persisted state. Hardcoded/echoed bodies, a constant `MessageId`, or a create that returns 200 without storing anything are failures of this issue. Errors must be emitted as the modeled rest-json error shapes with the HTTP statuses above (`AlreadyExistsException` 409, `BadRequestException` 400, `NotFoundException` 404, `LimitExceededException` 412, `TooManyRequestsException` 429, `InternalServiceErrorException` 500) — not bare 500s.

## State Model
Stored in the shared engine via `crate::state::service_state` (see `src/state.rs`):

- `configuration_sets: BTreeMap<String, ConfigurationSet>` — keyed by `ConfigurationSetName`; BTreeMap so ListConfigurationSets has a stable, resumable order for pagination tokens.
- `ConfigurationSet { name: String, created_at: SystemTime, event_destinations: BTreeMap<String, EventDestinationRecord> }`
- `EventDestinationRecord { name: String, enabled: bool, matching_event_types: Vec<String>, cloud_watch_logs: Option<CloudWatchLogsDest { iam_role_arn: Option<String>, log_group_arn: Option<String> }>, kinesis_firehose: Option<KinesisFirehoseDest { delivery_stream_arn: Option<String>, iam_role_arn: Option<String> }>, sns: Option<SnsDest { topic_arn: Option<String> }> }`
- `sent_messages: Vec<SentVoiceMessage { message_id: String, configuration_set_name: Option<String>, caller_id: Option<String>, destination_phone_number: Option<String>, origination_phone_number: Option<String>, content: VoiceContentRecord, sent_at: SystemTime }>` — appended by SendVoiceMessage; `message_id` from a monotonic counter/UUID so each call is distinct.

The remaining operations (UpdateConfigurationSetEventDestination, DeleteConfigurationSetEventDestination) stay 501 for now and will be filed as individual gap issues — though both become trivial once the state model above exists.

## Evidence

`OPERATIONS.md` (crate root) lists all 8 operations with routes, input/output members and errors: CreateConfigurationSet (`POST /v1/sms-voice/configuration-sets`, AlreadyExistsException 409), ListConfigurationSets (`GET /v1/sms-voice/configuration-sets`, NextToken/PageSize query params), CreateConfigurationSetEventDestination / GetConfigurationSetEventDestinations (`.../{ConfigurationSetName}/event-destinations`, NotFoundException 404), SendVoiceMessage (`POST /v1/sms-voice/voice/message` -> MessageId), DeleteConfigurationSet (`DELETE /v1/sms-voice/configuration-sets/{ConfigurationSetName}`). Shape members for `EventDestinationDefinition`, `EventDestination` (incl. `Name`) and `VoiceMessageContent` confirmed via smithy_query against `smithy-model/com-amazonaws-pinpointsmsvoice-pinpointsmsvoice.json`. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:'` matches all 8 files in `src/ops/`, so there is currently no behavior at all; no tests assert any of this (only the generated `tests/state_roundtrip.rs`).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `pinpoint-sms-voice` client at it (restJson1, signing name `sms-voice`).
2. CreateConfigurationSet(ConfigurationSetName="demo-set") -> expect 200 empty body; currently 501.
3. CreateConfigurationSet(ConfigurationSetName="demo-set") again -> expect AlreadyExistsException (HTTP 409).
4. ListConfigurationSets() -> expect ConfigurationSets to contain "demo-set".
5. CreateConfigurationSetEventDestination(ConfigurationSetName="demo-set", EventDestinationName="dest1", EventDestination={Enabled:true, MatchingEventTypes:["COMPLETED_CALL"], SnsDestination:{TopicArn:"arn:aws:sns:us-east-1:111122223333:voice-events"}}) -> expect 200.
6. GetConfigurationSetEventDestinations(ConfigurationSetName="demo-set") -> expect one EventDestination with Name="dest1", Enabled=true, MatchingEventTypes=["COMPLETED_CALL"] and the same SnsDestination.TopicArn.
7. GetConfigurationSetEventDestinations(ConfigurationSetName="no-such-set") -> expect NotFoundException (HTTP 404).
8. SendVoiceMessage(ConfigurationSetName="demo-set", DestinationPhoneNumber="+12065550100", OriginationPhoneNumber="+12065550199", Content={PlainTextMessage:{Text:"hello",LanguageCode:"en-US",VoiceId:"Matthew"}}) twice -> expect two 200s with two DIFFERENT non-empty MessageId values.
9. DeleteConfigurationSet(ConfigurationSetName="demo-set") -> expect 200.
10. ListConfigurationSets() -> expect "demo-set" absent; GetConfigurationSetEventDestinations("demo-set") -> expect NotFoundException (404); DeleteConfigurationSet("demo-set") again -> expect NotFoundException (404).
