Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pinpoint-sms-voice-v2.concurrent-create-configuration-set-same-name-one-winner
Issue-Shape-Id: com.amazonaws.pinpointsmsvoicev2#PinpointSMSVoiceV2
Issue-Key: pinpoint-sms-voice-v2.concurrent-create-configuration-set-same-name-one-winner|run#13|AssertionError|assert loser_message == expected_message, (
Issue-Commit: 6c5e636bebc82ccd4609eb5d54ba0275e4ea7ff4

# pinpoint-sms-voice-v2: CreateConfigurationSet's duplicate-name ConflictException message is emulator prose, not the service's 'Conflict Occurred - Reason=...' template

> _Note: this issue was identified against the code base at commit `6c5e636bebc82ccd4609eb5d54ba0275e4ea7ff4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfigurationSet

## Description
Cause 1 of 1: the ConflictException the emulator raises when CreateConfigurationSet is called for an already-existing configuration-set name carries an emulator-authored message instead of the message template End User Messaging SMS and Voice uses for every ConflictException.

AWS: ConflictException's message is a rendering of its own structured members. Smithy (com-amazonaws-pinpointsmsvoicev2#ConflictException): members Message, Reason (ConflictExceptionReason, which includes RESOURCE_ALREADY_EXISTS), ResourceType (ResourceType) and ResourceId; errorKind client, HTTP 400. The recording shows the Message rendered as the fixed text 'Conflict Occurred - Reason="<Reason>" ResourceType="<ResourceType>" ResourceId="<ResourceId>"' — i.e. the literal prefix 'Conflict Occurred - ' and the three quoted Reason=/ResourceType=/ResourceId= clauses are fixed, while the three quoted values are taken from the error's own members: Reason RESOURCE_ALREADY_EXISTS for a duplicate create, ResourceType 'configuration-set', and ResourceId the ConfigurationSetName from the request. The same service renders its other errors the same way ('Resource Not Found - ResourceType="configuration-set" ResourceId="<name>"'), so this is the service's message convention and not one recording's text. Error code ConflictException, HTTP 400.

Emulator: the code and HTTP status are right, but the message is 'configuration set <ConfigurationSetName> already exists' (observed: 'configuration set sim2real-behav-cfgset-75a57e744358 already exists'). services/service-com-amazonaws-pinpointsmsvoicev2-pinpointsmsvoicev2/src/ops/create_configuration_set.rs:217-227 sets reason/resource_type/resource_id correctly but formats message as format!("configuration set {name} already exists").

Why: a client that matches on the error message — log assertions, retry/branch logic keyed off the Reason clause, or a user-facing surface that echoes Message — sees text AWS never produces. Because the structured members are populated but the message is not derived from them, the divergence is invisible to a reader of the handler and shows up only at the client.

Fix: the ConflictException construction in this operation's handler (and the shared error-rendering helper it should use, so every ConflictException/ResourceNotFoundException in this service renders the one template) must build Message from Reason/ResourceType/ResourceId rather than from bespoke prose.

Traces: pinpoint-sms-voice-v2.concurrent-create-configuration-set-same-name-one-winner (two concurrent CreateConfigurationSet calls for one name: exactly one winner, and the loser's ConflictException message is compared verbatim). The open pinpoint-sms-voice (V1) issues listed by list_existing_issues concern a different service and do not cover this.

## Evidence
Expected (AWS, per the recording and the ConflictException model): code ConflictException, HTTP 400, Message 'Conflict Occurred - Reason="RESOURCE_ALREADY_EXISTS" ResourceType="configuration-set" ResourceId="<ConfigurationSetName>"'.
Actual (emulator): code ConflictException, HTTP 400, Message 'configuration set <ConfigurationSetName> already exists'.
Check failure: AssertionError: loser message 'configuration set sim2real-behav-cfgset-75a57e744358 already exists' did not match 'Conflict Occurred - Reason="RESOURCE_ALREADY_EXISTS" ResourceType="configuration-set" ResourceId="sim2real-behav-cfgset-75a57e744358"' (the id shown is this run's generated name, not part of the required text).
Handler: services/service-com-amazonaws-pinpointsmsvoicev2-pinpointsmsvoicev2/src/ops/create_configuration_set.rs:220 — message: Some(format!("configuration set {name} already exists")).

## Steps to Reproduce
1. Boot the emulator; client = botocore client for pinpoint-sms-voice-v2.
2. client.create_configuration_set(ConfigurationSetName="<name>") — succeeds.
3. client.create_configuration_set(ConfigurationSetName="<name>") again (or issue both concurrently from two threads).
4. Read the loser's error: Code is ConflictException as expected, but Message is 'configuration set <name> already exists' instead of 'Conflict Occurred - Reason="RESOURCE_ALREADY_EXISTS" ResourceType="configuration-set" ResourceId="<name>"'.
