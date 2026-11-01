Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: dbe80abdd81a79f1a95892fbee40e178463ee847

# audioChannelPairs is accepted with two configurations for the same audio pair, although the crate quotes the "one for each audio pair" sentence it enforces the rest of

_Filed: 2026-10-29T12:51:24Z_
_Operation: UpdateInputDevice | Class: validation_

> _Note: this issue was identified against the code base at commit `dbe80abdd81a79f1a95892fbee40e178463ee847`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateInputDevice` validates the `audioChannelPairs` list of `UhdDeviceSettings` through the input-device slice's one rule set, `crate::state::configurable_settings_error`. That function enforces two of the three things the member's own documentation states — at most eight configurations, and an id inside 1..=8 — but not the part the crate itself quotes beside the count: "one for each audio pair in the source". A list such as `[{"id":1,"profile":"CBR-AAC_HQ-192000"},{"id":1,"profile":"DISABLED"}]` passes every check (two entries is under the cap, and 1 is inside the id range), is stored verbatim by `InputDeviceConfiguration::apply`, and is then reported verbatim by the one device projection, so `DescribeInputDevice`/`ListInputDevices` report TWO audio configurations for audio pair 1 and none at all for the other seven pairs — a device configuration the model's "one for each audio pair" sentence does not describe, and from which no reader can tell which profile pair 1 actually carries.

The gap is visible in the refusal table of the crate's own test: the "one audio configuration more than a source has pairs" row is a nine-entry list in which `{"id": 8}` appears twice, and it is refused for its LENGTH alone — remove any one entry and the duplicate id is stored.

Related clause of the same sentence, same validator: the member is documented as applying "only when the device is configured as the source for a MediaConnect flow", and the crate's `uhd_only_member` comment quotes that very sentence with that clause elided (it keeps only "applies only to UHD devices" and the HD trailer), so `audioChannelPairs` is also accepted on a UHD device with no flow attached even though the handler already holds `device.configuration().mediaconnect`, the value it derives `outputType: MEDIACONNECT_FLOW` from. Note that this second half is currently pinned as ACCEPTED by tests/all/input_device_update.rs:169-202 (a row that carries `audioChannelPairs` with no `mediaconnectSettings`, labelled "the settings a uhd device can be configured with, including the uhd-only ones" and stating no reason about the flow clause), so the duplicate-id half above is the part nothing pins.

## Evidence

The modelled sentence — `smithy_query(query="doc", shape="InputDeviceConfigurableSettings", member="AudioChannelPairs")`:
```
An array of eight audio configurations, one for each audio pair in the source. Set up each audio configuration either to exclude the pair, or to format it and include it in the output from the device. This parameter applies only to UHD devices, and only when the device is configured as the source for a MediaConnect flow. For an HD device, you configure the audio by setting up audio selectors in the channel configuration.
```
and `InputDeviceConfigurableAudioChannelPairConfig$Id`: "The ID for one audio pair configuration, a value from 1 to 8."

The crate quotes the one-per-pair half of that sentence for its own constant — services/service-com-amazonaws-medialive-medialive/src/state/input_devices.rs:242-244:
```
/// How many audio configurations a device's `AudioChannelPairs` carries: "An
/// array of eight audio configurations, one for each audio pair in the source".
pub const MAX_AUDIO_CHANNEL_PAIRS: usize = 8;
```

and then enforces only the count and the id range — src/state/input_devices.rs:556-571:
```
    if let Some(pairs) = &settings.audio_channel_pairs {
        if pairs.len() > MAX_AUDIO_CHANNEL_PAIRS {
            return Some(format!(
                "AudioChannelPairs carries {} configurations; a source has {MAX_AUDIO_CHANNEL_PAIRS} audio pairs",
                pairs.len()
            ));
        }
        for id in pairs.iter().filter_map(|pair| pair.id) {
            if !(FIRST_AUDIO_CHANNEL_PAIR_ID..=LAST_AUDIO_CHANNEL_PAIR_ID).contains(&id) {
                return Some(format!(
                    "AudioChannelPairs names audio pair {id}; an audio pair id is a value from \
                     {FIRST_AUDIO_CHANNEL_PAIR_ID} to {LAST_AUDIO_CHANNEL_PAIR_ID}"
                ));
            }
        }
    }
```
while its own doc claims the function is where the settings' documented bounds live — src/state/input_devices.rs:538-543: "The rule lives in the slice rather than in the handler, so the bounds the settings' own documentation states are checked in one place: the members only a UHD device has ([`uhd_only_member`]), the audio pairs a source has ([`MAX_AUDIO_CHANNEL_PAIRS`], [`FIRST_AUDIO_CHANNEL_PAIR_ID`]) ...".

The duplicate is stored verbatim — src/state/input_devices.rs:495-505:
```
        if let Some(pairs) = settings.audio_channel_pairs {
            self.audio_channel_pairs = Some(
                pairs
                    .into_iter()
                    .map(|pair| InputDeviceUhdAudioChannelPairConfig {
                        id: pair.id,
                        profile: pair.profile,
                    })
                    .collect(),
            );
        }
```
and reported verbatim by the one device projection — src/state/input_devices.rs:1365: `audio_channel_pairs: configuration.audio_channel_pairs.clone(),`.

The handler path — src/ops/update_input_device.rs:229-239:
```
        if let Some(settings) = &settings {
            if let Some(message) = crate::state::configurable_settings_error(&device_type, settings)
            {
                return Err(UpdateInputDeviceErrorModeled::BadRequestException(
```

The elided clause — src/state/input_devices.rs:515-520 (`uhd_only_member`):
```
/// Each member's own documentation is what puts it here: `Codec`,
/// `InputResolution` and `MediaconnectSettings` are each "Only UHD devices can
/// specify this parameter", and `AudioChannelPairs` "applies only to UHD
/// devices" ("For an HD device, you configure the audio by setting up audio
/// selectors in the channel configuration").
```

Test coverage: the refusal table pins the two halves that ARE enforced and, in doing so, shows the duplicate slipping through on length alone — tests/all/input_device_update.rs:480-497:
```
        (
            "an audio pair id below the first one",
            uhd,
            json!({ "uhdDeviceSettings": { "audioChannelPairs": [{ "id": 0, "profile": "DISABLED" }] } }),
        ),
        (
            "an audio pair id past the last one",
            uhd,
            json!({ "uhdDeviceSettings": { "audioChannelPairs": [{ "id": 9, "profile": "DISABLED" }] } }),
        ),
        (
            "one audio configuration more than a source has pairs",
            uhd,
            json!({ "uhdDeviceSettings": { "audioChannelPairs": [
                { "id": 1 }, { "id": 2 }, { "id": 3 }, { "id": 4 },
                { "id": 5 }, { "id": 6 }, { "id": 7 }, { "id": 8 }, { "id": 8 },
            ] } }),
        ),
```
No row anywhere in tests/all/input_device_update.rs carries a short list that repeats an id, so nothing pins the current acceptance of one.

## Steps to Reproduce

1. POST /prod/inputDevices/uhd-0a1b2c3d4e5f6071/claim (any uhd- prefixed id) so the account holds a UHD device
2. PUT /prod/inputDevices/uhd-0a1b2c3d4e5f6071 with body {"uhdDeviceSettings":{"audioChannelPairs":[{"id":1,"profile":"CBR-AAC_HQ-192000"},{"id":1,"profile":"DISABLED"}]}}
3. Observe HTTP 200: the emulator accepts two configurations for audio pair 1. Expected, per the member's own sentence "An array of eight audio configurations, one for each audio pair in the source" (the sentence src/state/input_devices.rs:242-244 quotes for its cap) and by the same reading that refuses id 0, id 9 and a nine-entry list, a 400 BadRequestException naming the repeated pair
4. GET /prod/inputDevices/uhd-0a1b2c3d4e5f6071 — uhdDeviceSettings.audioChannelPairs reports both entries for pair 1 and nothing for pairs 2..8, so no reader can tell which profile pair 1 carries
5. Compare with PUT ...{"uhdDeviceSettings":{"audioChannelPairs":[{"id":1},{"id":2},{"id":3},{"id":4},{"id":5},{"id":6},{"id":7},{"id":8},{"id":8}]}} -> 400, refused only because the list is nine long (tests/all/input_device_update.rs:490-497), although it repeats pair 8 exactly as step 2 repeats pair 1
6. Secondary clause: PUT ...{"uhdDeviceSettings":{"audioChannelPairs":[{"id":1,"profile":"DISABLED"}]}} on a UHD device with no mediaconnectSettings attached -> 200, although the member applies "only when the device is configured as the source for a MediaConnect flow"; this acceptance is currently pinned by tests/all/input_device_update.rs:169-202
