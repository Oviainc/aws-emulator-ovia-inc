Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3ebf0e2f135a02ae75ffd55898f9ee21fc25614a

# The switching and CMSD-header defaults are materialised onto channels whose InputType may not carry them, so a channel reports members the same service refuses to accept on it

_Filed: 2026-10-28T20:14:18Z_
_Operation: CreateChannel | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3ebf0e2f135a02ae75ffd55898f9ee21fc25614a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateChannel materialises three channel members the model gives a service-applied value: `OutputLockingMode`, `InputSwitchConfiguration.MQCSInputSwitching` and `OutputHeaderConfiguration.PublishMQCS`. For the first one the crate gates the materialisation on the channel's input type and says why; for the other two it does not, so EVERY channel - including an HLS channel, which is the default input type, and a MULTIVIEW channel - is stored and reported with `InputSwitchConfiguration: {MQCSInputSwitching: false}` and `OutputHeaderConfiguration: {PublishMQCS: false}`.

The same handler refuses those two members on exactly those channels. `crate::state::channel_input_type_violation` restricts both to `InputType == CMAF` and the check is presence-based (`submitted_members` uses `.is_some()`), so a request that sends `MQCSInputSwitching: false` on an HLS channel is answered 400 ValidationException / ONLY_CMAF_INPUT_TYPE_ALLOW_MQCS_INPUT_SWITCHING. The result is a self-contradiction that needs no knowledge of AWS: the service reports a configuration that it will not accept back. A caller that reads a channel (or a `ListChannels` projection) and replays the reported configuration into a create or an `UpdateChannel` is refused for sending the value the service just handed it, while `OutputLockingMode` - restricted the same way, through the same table - is correctly reported as absent on the very same HLS channel.

A second, smaller half: `PublishMQCS` has no documented default at all (its doc carries only the CMAF restriction), yet it is materialised to `false`. The crate applies the opposite criterion one field away: `PreferredInput` is deliberately left absent because it "has no documented default and the service reports none".

UpdateChannel re-materialises both blocks through the same two helpers (src/ops/update_channel.rs:371-376), so no write ever leaves a non-CMAF channel without them.

## Evidence

Smithy - `InputSwitchConfiguration$MQCSInputSwitching` (smithy_query doc, services/service-com-amazonaws-mediapackagev2-mediapackagev2/smithy-model/com-amazonaws-mediapackagev2-mediapackagev2.json):
```
When true, AWS Elemental MediaPackage performs input switching based on the MQCS. Default is false. This setting is valid only when InputType is CMAF.
```
and `OutputHeaderConfiguration$PublishMQCS`:
```
When true, AWS Elemental MediaPackage includes the MQCS in responses to the CDN. This setting is valid only when InputType is CMAF.
```
(no default sentence on the second one).

The rule the crate states for exactly this situation - src/state.rs:1302-1326 (`materialized_output_locking_mode`):
```
/// The model documents a service-applied default for the member ("If you don't specify a
/// value, the default is `EPOCH_LOCKED`") AND restricts which input types may carry it at
/// all, so the default is materialised only on a channel whose input type may carry it; a
/// channel that may not has no mode and reports none. The restriction is read off the
/// same table a SUBMITTED mode is refused by ([`channel_input_type_violation`]), so a
/// stored mode and a refused one can never disagree about which input types carry the
/// member.
pub fn materialized_output_locking_mode(submitted: Option<String>, input_type: &str) -> Option<String> {
```

The two siblings that break it - src/state.rs:1272-1284, neither of which even takes an input type:
```
pub fn materialized_input_switch_configuration(
    submitted: Option<InputSwitchConfiguration>,
) -> InputSwitchConfiguration {
    let mut configuration = submitted.unwrap_or_default();
    configuration.mqcs_input_switching = Some(configuration.mqcs_input_switching.unwrap_or(false));
    configuration
}
```
```
/// The output-header configuration a channel is stored with - the same materialisation
/// as [`materialized_input_switch_configuration`], one member wide: CMSD header
/// publication is off unless the caller turned it on.
pub fn materialized_output_header_configuration(
```
and the criterion they ignore, stated one line above them for `PreferredInput` - src/state.rs:1270-1271: "`PreferredInput` has no documented default and the service reports none, so an omitted one stays omitted."

The refusal side, same crate, same members - src/state.rs:2721-2744:
```
    ChannelInputTypeRule {
        member: "inputSwitchConfiguration.mqcsInputSwitching",
        allowed: &["CMAF"],
        exception_type: "ONLY_CMAF_INPUT_TYPE_ALLOW_MQCS_INPUT_SWITCHING",
...
    ChannelInputTypeRule {
        member: "outputHeaderConfiguration.publishMqcs",
        allowed: &["CMAF"],
        exception_type: "ONLY_CMAF_INPUT_TYPE_ALLOW_MQCS_OUTPUT_CONFIGURATION",
```
while the row above it records why `outputLockingMode` is exempt: "// The member has a documented DEFAULT, so an absent one is materialised rather than refused: see `materialized_output_locking_mode`." (src/state.rs:2715-2717).

The handler - src/ops/create_channel.rs:474-479 materialises both blocks with no input type in sight:
```
        let input_switch_configuration = crate::state::materialized_input_switch_configuration(
            input.input_switch_configuration,
        );
        let output_header_configuration = crate::state::materialized_output_header_configuration(
            input.output_header_configuration,
        );
```
while src/ops/create_channel.rs:485-489 does gate the locking mode (`materialized_output_locking_mode(input.output_locking_mode, &input_type)`), and src/ops/create_channel.rs:301-302 reports both blocks unconditionally (`input_switch_configuration: Some(...)`, `output_header_configuration: Some(...)`).

Test coverage pins BOTH sides of the contradiction. The refusal - tests/all/control_plane_validation.rs:313-336:
```
        InputTypeCase {
            label: "MQCS input switching on HLS input",
            input_type: Some("HLS"),
            members: || json!({"InputSwitchConfiguration": {"MQCSInputSwitching": true}}),
            refused_as: Some("ONLY_CMAF_INPUT_TYPE_ALLOW_MQCS_INPUT_SWITCHING"),
        },
...
            label: "PublishMQCS on HLS input",
            refused_as: Some("ONLY_CMAF_INPUT_TYPE_ALLOW_MQCS_OUTPUT_CONFIGURATION"),
```
The report - tests/all/server_materialised_members.rs:61-71, whose channel is created with `json!({})`, i.e. the HLS default (tests/all/server_materialised_members.rs:33):
```
        Case {
            label: "no configuration blocks at all",
            body: || json!({}),
            expected: vec![
                ("/InputSwitchConfiguration/MQCSInputSwitching", json!(false)),
                ("/InputSwitchConfiguration/PreferredInput", Value::Null),
                ("/OutputHeaderConfiguration/PublishMQCS", json!(false)),
            ],
        },
```
Its stated reason quotes only half of the member's doc - tests/all/server_materialised_members.rs:56-58: "the booleans inside them default to `false` (the model's `MQCSInputSwitching` doc states \"Default is false\")" - and the next sentence of that same doc ("This setting is valid only when InputType is CMAF") is what the row contradicts, so this row is the one that has to change with the fix.

## Steps to Reproduce

1. Boot the emulator. POST /channelGroup with {"ChannelGroupName":"g"}.
2. POST /channelGroup/g/channel with {"ChannelName":"c"} - no InputType, so the channel is HLS (state::DEFAULT_INPUT_TYPE).
3. Observe the 200 body: it reports InputSwitchConfiguration {"MQCSInputSwitching": false} and OutputHeaderConfiguration {"PublishMQCS": false}, and NO OutputLockingMode (correctly withheld, because HLS may not carry it). GET /channelGroup/g/channel/c and GET /channelGroup/g/channel report the same two blocks.
4. Now replay what was just reported: POST /channelGroup/g/channel with {"ChannelName":"c2","InputSwitchConfiguration":{"MQCSInputSwitching":false}}. Observe 400 ValidationException / ONLY_CMAF_INPUT_TYPE_ALLOW_MQCS_INPUT_SWITCHING - the service refuses the value it reported in step 3. Same with {"OutputHeaderConfiguration":{"PublishMQCS":false}} -> ONLY_CMAF_INPUT_TYPE_ALLOW_MQCS_OUTPUT_CONFIGURATION.
5. Repeat step 2 with InputType MULTIVIEW (plus the MultiviewConfiguration that input type requires) for the same two blocks on a channel that receives no ingest at all.
6. PUT /channelGroup/g/channel/c with {} (UpdateChannel) and confirm both blocks are re-materialised on the stored channel by the same two helpers.
