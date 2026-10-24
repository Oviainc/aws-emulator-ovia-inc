Issue-Severity: HIGH
Issue-Gap-Class: validation
Issue-Commit: d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f

# CreateJob accepts outputs whose Preset reference resolves to nothing, and outputs that set both Preset and ContainerSettings

_Filed: 2026-10-24T13:28:46Z_
_Operation: CreateJob | Class: validation_

> _Note: this issue was identified against the code base at commit `d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An Output inside Settings.OutputGroups[].Outputs[] is documented as carrying EITHER a preset reference OR its own container settings: `Output$Preset` is "Use Preset to specify a preset for your transcoding settings. Provide the system or custom preset name. You can specify either Preset or Container settings, but not both."

The emulator validates neither half of that rule, and never resolves the reference at all:

1. A job whose output names a preset the account does not hold (a typo, a preset deleted after the template was written, a system preset the emulator does not seed) is accepted with HTTP 201 and a full Job body. Expected: MediaConvert resolves the preset at submit time and refuses the request with BadRequestException (400) naming the preset; no job is created. Actual: the job is stored and the dangling preset name is echoed back by GetJob/ListJobs forever.
2. An output that sets BOTH `preset` and `containerSettings` is accepted, where the member's own documentation says the two are mutually exclusive.
3. An output that sets NEITHER is likewise accepted, so the job describes an output with no encoder configuration at all.

All three are refusals the real service makes synchronously at submit time (that is what the error envelope's `settingsValidationErrorsJsonBlob` member reports), so the emulator is over-permissive: a client validating its wiring gets a green 201 where AWS returns 400.

## Evidence

Smithy model: `Output$Preset` (target __stringMin0, jsonName "preset") — "Provide the system or custom preset name. You can specify either Preset or Container settings, but not both."; `Output$ContainerSettings`; CreateJob declares BadRequestException (httpError 400). Handler: services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/create_job.rs — the only settings check is `crate::state::job_settings_problem(&settings, template)`, which (src/state.rs, `job_settings_problem`, JobSettingsProblem::{NoInputs,NoOutputGroups}) only counts that `inputs` and `output_groups` are non-empty. Nothing walks the outputs. A repo-wide grep for the `Output.preset` field (src/ops/types/shared_types.rs line ~978) shows it is read by NO handler and by no helper in state.rs — the preset store (`state.presets`) is never consulted on a CreateJob path. Test coverage: tests/all/create_job_validation.rs covers only the empty-inputs / empty-output-groups and template-reference cases; no test submits an output with a preset reference, so nothing asserts this either way.

## Steps to Reproduce

1. Boot the emulator and point a mediaconvert client at it.
2. create_job(Role=<role>, Settings={'Inputs':[{'FileInput':'s3://bucket/in.mp4'}],'OutputGroups':[{'OutputGroupSettings':{'Type':'FILE_GROUP_SETTINGS','FileGroupSettings':{'Destination':'s3://bucket/out/'}},'Outputs':[{'Preset':'no-such-preset-xyz','NameModifier':'_1'}]}]}).
3. Expect BadRequestException (400) naming the missing preset and list_jobs() to stay empty; observe HTTP 201 with a SUBMITTED Job that carries the dangling preset name, visible again from get_job(Id=...).
4. Repeat with an output that sets BOTH 'Preset':'<an existing preset>' and 'ContainerSettings':{'Container':'MP4'} — expect a 400 for the mutually exclusive pair, observe 201.
5. Repeat with an output that sets neither Preset nor ContainerSettings — expect a 400, observe 201.
