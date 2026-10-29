Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 060392db00c5b3260ef85f15c64a4c64c2352242

# The jobs-query audioCodec/videoCodec filters read only an output's inline codec settings, so an output that names a preset never matches

_Filed: 2026-10-29T05:00:28Z_
_Operation: StartJobsQuery | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `060392db00c5b3260ef85f15c64a4c64c2352242`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A jobs query's `audioCodec` and `videoCodec` filter keys are evaluated against the job's own `Settings.OutputGroups[].Outputs[].AudioDescriptions[].CodecSettings.Codec` and `...VideoDescription.CodecSettings.Codec` only. But the crate itself enforces that an output states its encoder settings EXACTLY ONCE — either inline (its own `ContainerSettings`) or by naming a preset — so for every output that names a preset, the inline descriptions the filter reads are necessarily absent and `audio_codecs()`/`video_codecs()` yield nothing.

The consequence is a silent empty result: `StartJobsQuery` with `filterList: [{key: "audioCodec", values: ["AAC"]}]` reports no jobs at all for an account whose jobs take their encoding from presets, even though the preset the output names stores exactly that codec in this very crate (`PresetRecord.settings.audio_descriptions[].codec_settings.codec` / `.video_description.codec_settings.codec`, which the crate's own built-in preset catalogue fills in for every seeded preset).

The reference IS resolvable at the point the match runs: `accept_jobs_query` runs the match on `&mut self` of the state that holds `self.presets`, and the sibling validation path `output_problem` already resolves `Output$Preset` against `self.presets` on the very same graph. Only this read of the graph stops at the output and never follows the preset.

The gap is `wrong-behavior`, not a missing feature: the crate does implement these two filter keys (`audio_codecs()`/`video_codecs()` exist for nothing else) — the walk they do is just incomplete, and it turns a narrowing filter into one that matches nothing.

## Evidence

Smithy (`smithy_query(query="doc",shape="JobsQueryFilterKey")`) states what the two keys filter on:
  "* audioCodec - Your output's audio codec. (AAC | MP2 | MP3 | WAV | AIFF | AC3| EAC3 | EAC3_ATMOS | VORBIS | OPUS | PASSTHROUGH | FLAC) * videoCodec - Your output's video codec. (AV1 | AVC_INTRA | FRAME_CAPTURE | H_264 | H_265 | MPEG2 | PASSTHROUGH | PRORES | UNCOMPRESSED | VC3 | VP8 | VP9 | XAVC)"

The handler path reads only the inline descriptions — services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs:3218-3219:
```
            JOBS_QUERY_FILTER_KEY_AUDIO_CODEC => self.audio_codecs().any(|codec| codec == value),
            JOBS_QUERY_FILTER_KEY_VIDEO_CODEC => self.video_codecs().any(|codec| codec == value),
```
state.rs:3262-3276:
```
    /// The `Codec` of every audio description of every output of this job.
    fn audio_codecs(&self) -> impl Iterator<Item = &str> + '_ {
        self.outputs()
            .flat_map(|output| output.audio_descriptions.iter().flatten())
            .filter_map(|description| description.codec_settings.as_ref())
            .filter_map(|settings| settings.codec.as_deref())
    }

    /// The `Codec` of every output's video description.
    fn video_codecs(&self) -> impl Iterator<Item = &str> + '_ {
        self.outputs()
            .filter_map(|output| output.video_description.as_ref())
            ...
    }
```

The SAME crate states that an output which names a preset carries no inline settings of its own — state.rs:1301-1310 (doc of `output_problem`):
```
    /// `Output$Preset`: "Provide the system or custom preset name. You can specify either
    /// Preset or Container settings, but not both" — so an output states its encoding
    /// settings EXACTLY once, either by naming a preset or by carrying container settings
    /// whose `Container` says what the output is ... A name is a reference, so the one an
    /// output does state is resolved against the account's presets here
```
and state.rs:1329-1336 refuses an output that states both:
```
        if container {
            return Some(SettingsProblem::BothPresetAndContainer { group, output });
        }
        (!self.presets.contains_key(preset)).then(|| SettingsProblem::PresetNotFound {
```

The codecs the filter needs ARE stored by this crate for a preset-backed output — state.rs:2151-2170 (`SystemPreset::settings`):
```
            video_description: Some(VideoDescription {
                ...
                codec_settings: Some(video_codec_settings(self.video_codec, ...)),
            }),
            audio_descriptions: Some(vec![aac_audio_description(self.audio_bitrate)]),
```
and the model confirms `PresetSettings` carries `AudioDescriptions` and `VideoDescription`.

The handler's own stated intent, which this breaks — state.rs:3195-3199:
```
    /// Each `JobsQueryFilterKey` is read off THIS record — the same state [`Self::view`]
    /// reports — so a query matches on what a reader of the job would see, never on the
    /// text of the request that created it.
```

Test coverage: services/service-com-amazonaws-mediaconvert-mediaconvert/tests/all/jobs_query.rs:338-348 exercises `audioCodec`/`videoCodec` only against fixture jobs whose outputs carry INLINE codec settings (`a_jobs_query_matches_on_any_value_of_any_filter`); no case drives a job whose output names a preset, so the gap is unpinned. The preset-backed output shape itself is already exercised elsewhere — tests/all/preset_update_container_integrity.rs:414-441 creates a job whose only output is `{ "nameModifier": "_1", "preset": name }`.

## Steps to Reproduce

1. POST /2017-08-29/presets with {"name":"aac-h264-mp4","settings":{"containerSettings":{"container":"MP4"},"videoDescription":{"codecSettings":{"codec":"H_264"}},"audioDescriptions":[{"codecSettings":{"codec":"AAC"}}]}} -> 201
2. POST /2017-08-29/jobs with {"role":"<role>","settings":{"inputs":[{"fileInput":"s3://example-bucket/in/a.mp4"}],"outputGroups":[{"name":"File Group","outputGroupSettings":{"type":"FILE_GROUP_SETTINGS","fileGroupSettings":{"destination":"s3://example-bucket/out/"}},"outputs":[{"nameModifier":"_1","preset":"aac-h264-mp4"}]}]}} -> 201 (the output must NOT carry inline ContainerSettings: stating both is refused with BadRequestException)
3. POST /2017-08-29/jobsQueries with {"filterList":[{"key":"audioCodec","values":["AAC"]}]} -> 201, note the returned Id
4. GET /2017-08-29/jobsQueries/{Id} until status is COMPLETE
5. Observe: "jobs": [] — the job whose output's audio codec is AAC (via the preset it names) is not matched. Repeat step 3 with {"key":"videoCodec","values":["H_264"]} for the same empty result
6. Control: submit a second job that spells the same codecs out inline (containerSettings + audioDescriptions/videoDescription on the output) and re-run the query — only that job comes back, showing the filter reads the inline graph only
