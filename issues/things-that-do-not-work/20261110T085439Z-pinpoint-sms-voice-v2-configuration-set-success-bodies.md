Issue-Source: trace-sweep-divergence
Issue-Trace: pinpoint-sms-voice-v2-configuration-set-client-token-idempotency
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pinpoint-sms-voice-v2-configuration-set-client-token-idempotency pinpoint-sms-voice-v2-configuration-set-lifecycle pinpoint-sms-voice-v2-configuration-set-lifecycle-and-conflict pinpoint-sms-voice-v2-protect-configuration-deletion-protection-and-single-association pinpoint-sms-voice-v2-tag-resource-lifecycle
Issue-Shape-Id: com.amazonaws.pinpointsmsvoicev2#PinpointSMSVoiceV2
Issue-Key: pinpoint-sms-voice-v2|CreateConfigurationSet|2xx->2xx|$.CreatedTimestamp
Issue-Key: pinpoint-sms-voice-v2|DeleteConfigurationSet|2xx->2xx|$.CreatedTimestamp
Issue-Key: pinpoint-sms-voice-v2|DeleteConfigurationSet|2xx->2xx|$.DefaultMessageFeedbackEnabled
Issue-Key: pinpoint-sms-voice-v2|DeleteConfigurationSet|2xx->2xx|$.DefaultMessageType
Issue-Key: pinpoint-sms-voice-v2|DeleteConfigurationSet|2xx->2xx|$.DefaultSenderId
Issue-Commit: a11b28e572acb01424c781dc62b11ca84bce3a9b

# pinpoint-sms-voice-v2 configuration-set success bodies diverge: CreatedTimestamp precision and unstored defaults

> _Note: this issue was identified against the code base at commit `a11b28e572acb01424c781dc62b11ca84bce3a9b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteConfigurationSet

## Description
This bundle carries 2 causes spread over 3 divergence clusters, all in the 200 bodies of the configuration-set operations. A fixer must close both before submitting.

Cause 1 of 2 — CreatedTimestamp is serialized as a sub-second (microsecond-precision) epoch value, which fails the fixtures' shape assertion for the member. Two clusters: CreateConfigurationSet and DeleteConfigurationSet.
AWS: Smithy models CreateConfigurationSetResult.CreatedTimestamp and DeleteConfigurationSetResult.CreatedTimestamp as Timestamp; the operation docs say 'The time when the configuration set was created, in UNIX epoch time format', and the protocol is awsJson1_0, so the wire form is an epoch-seconds number. The recordings do not assert the instant (the fixture records it as <shaped-not-asserted>), only the member's shape, so this is recorded behavior only: AWS's recorded configuration-set CreatedTimestamp values carry no microsecond tail, while the emulator's do. Hypothesis: the service stamps the record at second (at most millisecond) granularity, so a value with 7 fractional digits is a form AWS does not emit.
Emulator: both operations return CreatedTimestamp as a float carrying the full sub-second remainder of the host clock — observed 1794300627.7584016 on create and 1794300622.8734586 on delete. (The instants themselves are per-run values and are NOT the finding; the fractional precision of the serialized form is.) Source: crate::state::to_timestamp builds the DateTime with DateTime::from_secs_f64(duration.as_secs_f64()), so every stamped timestamp inherits the raw clock's sub-second remainder.
Why: a client that round-trips, hashes, compares or displays CreatedTimestamp (and any consumer that assumes the service's granularity, e.g. keying a cache or diffing a describe against a create) sees a value shaped unlike anything the real service returns; a strict deserializer or a fixture asserting the member's shape rejects it.
Fix: the shared timestamp helper in this service's state module (and therefore every member it stamps) should truncate to the granularity the service emits before handing the DateTime to the serializer.
Traces: pinpoint-sms-voice-v2-configuration-set-client-token-idempotency#task#1 (CreateConfigurationSet; cluster also covers pinpoint-sms-voice-v2-configuration-set-lifecycle#task#1, pinpoint-sms-voice-v2-configuration-set-lifecycle-and-conflict#task#1 and pinpoint-sms-voice-v2-protect-configuration-deletion-protection-and-single-association#task#5) and pinpoint-sms-voice-v2-configuration-set-lifecycle#task#5 (DeleteConfigurationSet; cluster also covers pinpoint-sms-voice-v2-protect-configuration-deletion-protection-and-single-association#task#12 and pinpoint-sms-voice-v2-tag-resource-lifecycle#task#8). Helper: services/service-com-amazonaws-pinpointsmsvoicev2-pinpointsmsvoicev2/src/state.rs:84-89.

Cause 2 of 2 — DeleteConfigurationSet's response does not report the set's configured defaults: DefaultMessageType and DefaultSenderId come back absent and DefaultMessageFeedbackEnabled comes back false after they were set.
AWS: Smithy DeleteConfigurationSetResult carries ConfigurationSetArn, ConfigurationSetName, EventDestinations, DefaultMessageType (MessageType), DefaultSenderId (SenderId, len 1..11, pattern ^[A-Za-z0-9_-]+$), DefaultMessageFeedbackEnabled (Boolean) and CreatedTimestamp. The recording shows the delete echoing the state the set held at deletion: a set whose message type, sender id and message-feedback flag had been set reports those values back (the recorded run had DefaultMessageType 'Transactional', DefaultSenderId 'NOTICE', DefaultMessageFeedbackEnabled true — the particular values come from the earlier Set* calls in the fixture and are not themselves the requirement). HTTP 200.
Emulator: HTTP 200 with DefaultMessageType and DefaultSenderId absent and DefaultMessageFeedbackEnabled false. The delete handler does read all three off the stored record (src/ops/delete_configuration_set.rs:192-200), so the gap is that nothing ever writes them: SetDefaultMessageType, SetDefaultSenderId and SetDefaultMessageFeedbackEnabled are still scaffold stubs (e.g. src/ops/set_default_message_type.rs is marked SCAFFOLDED, NOT IMPLEMENTED), so the ConfigurationSet record's default_message_type / default_sender_id / default_message_feedback_enabled fields keep their create-time None/None/false forever.
Why: a client that configures a configuration set and then reads it back — on delete, or on any describe that reports the same members — is told the set has no defaults, so configuration it successfully applied is invisible and any assertion or decision based on it is wrong.
Fix: the three Set* handlers must persist into the configuration-set record in this service's state so the delete (and describe) path reports them; the delete handler's own projection is already correct. The per-operation stub issues for those Set* operations cover standing them up at all, but not this response-fidelity divergence, which is what this cause records.
Traces: pinpoint-sms-voice-v2-configuration-set-lifecycle-and-conflict#task#8.

## Evidence
Cause 1, cluster A — fixture pinpoint-sms-voice-v2-configuration-set-client-token-idempotency step task#1, CreateConfigurationSet, HTTP 200 expected and returned. $.CreatedTimestamp: expected <shaped-not-asserted> (the member's shape, not its instant), got 1794300627.7584016. Affected: pinpoint-sms-voice-v2-configuration-set-lifecycle#task#1, pinpoint-sms-voice-v2-configuration-set-lifecycle-and-conflict#task#1, pinpoint-sms-voice-v2-protect-configuration-deletion-protection-and-single-association#task#5.

Cause 1, cluster B — fixture pinpoint-sms-voice-v2-configuration-set-lifecycle step task#5, DeleteConfigurationSet, HTTP 200 expected and returned. $.CreatedTimestamp: expected <shaped-not-asserted>, got 1794300622.8734586. Affected: pinpoint-sms-voice-v2-protect-configuration-deletion-protection-and-single-association#task#12, pinpoint-sms-voice-v2-tag-resource-lifecycle#task#8. Code: src/state.rs:84-89 — DateTime::from_secs_f64(since_epoch.as_secs_f64()).

Cause 2 — fixture pinpoint-sms-voice-v2-configuration-set-lifecycle-and-conflict step task#8, DeleteConfigurationSet, HTTP 200 expected and returned. $.DefaultMessageFeedbackEnabled: expected true, got false. $.DefaultMessageType: expected 'Transactional', got None (member absent). $.DefaultSenderId: expected 'NOTICE', got None (member absent). Code: src/ops/delete_configuration_set.rs:196-198 reads the record's fields; src/ops/create_configuration_set.rs:255-257 initialises them to None/None/false; src/ops/set_default_message_type.rs (and the sender-id / message-feedback equivalents) are SCAFFOLDED, NOT IMPLEMENTED, so no path ever writes them. All paths under services/service-com-amazonaws-pinpointsmsvoicev2-pinpointsmsvoicev2/.

## Steps to Reproduce
Boot the emulator and point an sms-voice (pinpoint-sms-voice-v2) client at it.

Cause 1: create_configuration_set(ConfigurationSetName='ts-probe') and read CreatedTimestamp off the raw awsJson1_0 body — it carries a microsecond-level fractional tail (e.g. ...627.7584016) rather than the granularity the service emits. Then delete_configuration_set(ConfigurationSetName='ts-probe') and observe the same form in the delete response.

Cause 2: create_configuration_set(ConfigurationSetName='defaults-probe'); call set_default_message_type(ConfigurationSetName='defaults-probe', MessageType='TRANSACTIONAL'), set_default_sender_id(..., SenderId='NOTICE') and set_default_message_feedback_enabled(..., MessageFeedbackEnabled=True) (today each answers 501); then delete_configuration_set(ConfigurationSetName='defaults-probe') and observe DefaultMessageType/DefaultSenderId absent and DefaultMessageFeedbackEnabled false instead of the configured values.

Re-replay: pinpoint-sms-voice-v2-configuration-set-client-token-idempotency#task#1, pinpoint-sms-voice-v2-configuration-set-lifecycle#task#5, pinpoint-sms-voice-v2-configuration-set-lifecycle-and-conflict#task#8.
