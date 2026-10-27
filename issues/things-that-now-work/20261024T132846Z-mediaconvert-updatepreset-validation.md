Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f

# UpdatePreset replaces Settings with no validation, so a preset can be left describing no output container

_Filed: 2026-10-24T13:28:46Z_
_Operation: UpdatePreset | Class: validation_

> _Note: this issue was identified against the code base at commit `d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdatePreset swaps the whole PresetSettings graph in whenever the request carries one, with no test of its contents. A preset exists to supply an output's encoder configuration — `Output$Preset` says a preset is used "to specify a preset for your transcoding settings" and is mutually exclusive with an output's own container settings, so a preset with no ContainerSettings describes no output the service could produce. The real service refuses such a graph with BadRequestException (400) and leaves the stored preset untouched.

Expected: UpdatePreset(Name=<existing>, Settings={}) (or any graph with no ContainerSettings) is a 400 and GetPreset still reports the previous settings. Actual: HTTP 200, the empty graph is persisted, LastUpdated moves, and every later GetPreset — and every job that references the preset — sees a preset that configures nothing.

(An already-open issue covers the identical missing check on CreatePreset; this operation is a second, independent door to the same invalid state and is not covered by it.)

## Evidence

Smithy model: `PresetSettings` members are AudioDescriptions / CaptionDescriptions / ContainerSettings / VideoDescription, with `ContainerSettings` the one that names the output container; `Output$Preset` — "You can specify either Preset or Container settings, but not both"; UpdatePreset (PUT /2017-08-29/presets/{Name} -> 200) declares BadRequestException httpError 400. Handler: src/ops/update_preset.rs — the only refusals are the 404 for an unknown name and the 403 for a SYSTEM preset; `if let Some(settings) = input.settings { record.settings = settings; }` then applies the graph unconditionally and `record.last_updated = now`. Test coverage: tests/all/preset_update.rs and preset_lifecycle.rs only drive well-formed PresetSettings through the update path; nothing asserts a refusal.

## Steps to Reproduce

1. Boot the emulator and point a mediaconvert client at it.
2. create_preset(Name='p1', Settings={'ContainerSettings':{'Container':'MP4'},'VideoDescription':{'CodecSettings':{'Codec':'H_264'}}}).
3. update_preset(Name='p1', Settings={}).
4. Expect BadRequestException (400) and get_preset(Name='p1') still reporting ContainerSettings.Container 'MP4' with the original LastUpdated; observe HTTP 200, a preset whose settings are empty, and a moved LastUpdated.
5. Reference the now-empty preset from a CreateJob output to see the invalid preset accepted downstream.
