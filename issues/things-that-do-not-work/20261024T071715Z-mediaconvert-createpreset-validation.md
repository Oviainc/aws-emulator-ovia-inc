Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# CreatePreset accepts a PresetSettings graph that describes no output at all (no ContainerSettings)

_Filed: 2026-10-24T07:17:15Z_
_Operation: CreatePreset | Class: validation_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A preset is the encoder-settings graph for ONE output, and MediaConvert validates that graph when the preset is created — the service's own error envelope carries `settingsValidationErrorsJsonBlob` for exactly this class of refusal. At minimum the settings must name the output's container (`PresetSettings$ContainerSettings` / `ContainerSettings$Container`).

Expected: `CreatePreset` with `Settings: {}` (or settings that name no container) is refused with BadRequestException (400) naming the missing settings member, and nothing is stored — the same shape of refusal this emulator already applies to a CreateJob whose Settings describe no inputs or outputs (state::job_settings_problem).

Actual: ops/create_preset.rs validates only that `Name` is non-blank; the whole `Settings` value is persisted verbatim however empty it is, so `CreatePreset(Name='p', Settings={})` answers 201 and GetPreset/ListPresets report a preset that describes no output. `UpdatePreset` has the same hole: `record.settings = settings` replaces a valid graph with an empty one and answers 200.

## Evidence

Spec: CreatePreset models `Settings` (PresetSettings) as @required and BadRequestException (400); `PresetSettings` members are AudioDescriptions / CaptionDescriptions / ContainerSettings / VideoDescription ('Settings for preset'), and `ContainerSettings$Container` is 'Container for this output'. MediaConvert performs settings-graph validation at create time — the service error envelope member `settingsValidationErrorsJsonBlob` exists to report it (see the fixed issue 20261022T124827Z-mediaconvert-error-responses-omit-the-service-s-error about that envelope). Precedent in this repo: the analogous template-plane gap is open as 20261024T051247Z-mediaconvert-createjobtemplate-validation ('accepts a Settings graph that describes no outputs'), and CreateJob already refuses an empty graph. Handler: services/.../src/ops/create_preset.rs (name/category/description only; `settings: input.settings` stored unchecked) and src/ops/update_preset.rs (`record.settings = settings`). Test coverage: tests/all/preset_lifecycle.rs case 'empty settings object: no member of PresetSettings given' (line ~235) and preset_update.rs (line ~205) currently assert that the empty graph is ACCEPTED — the current behaviour is enshrined by those cases and they would need revising with the fix; the emulator's own fixtures call `containerSettings.container` 'the one member every preset must describe' (tests/all/idempotent_repeats.rs line ~44).

## Steps to Reproduce

1. Boot the emulator and point a botocore mediaconvert client at it.
2. create_preset(Name='p-empty', Settings={}) -> expect BadRequestException (400) naming the missing container/output settings and no preset stored; observe HTTP 201 with a Preset body.
3. get_preset(Name='p-empty') -> observe a stored preset whose settings describe no output.
4. create_preset(Name='p-ok', Settings={'ContainerSettings': {'Container': 'MP4'}}) then update_preset(Name='p-ok', Settings={}) -> expect the same 400; observe HTTP 200 and a preset whose settings are now empty.
