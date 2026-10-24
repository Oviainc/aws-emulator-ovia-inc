Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2cb6c27e7920aa8b3ecbca8018912df677b4544f

# CreateJobTemplate accepts a Settings graph that describes no outputs

_Filed: 2026-10-24T05:12:47Z_
_Operation: CreateJobTemplate | Class: validation_

> _Note: this issue was identified against the code base at commit `2cb6c27e7920aa8b3ecbca8018912df677b4544f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateJobTemplate accepts any JobTemplateSettings structure, including `"settings": {}` or a settings object with an empty `OutputGroups` list, answering 201 and persisting a template that can never produce a job. The real service runs its settings validator on the template at create time and refuses such a request with BadRequestException (400) — the same validator that refuses a CreateJob whose settings describe no outputs, which is why the service's error envelope carries `settingsValidationErrorsJsonBlob`.

Expected: a job template whose Settings carry no OutputGroups (or an output group with no Outputs, or an output with no ContainerSettings) is a 400 naming the settings problem and nothing is stored. Actual: `src/ops/create_job_template.rs` validates only that `Name` is non-empty and that a named `Queue` resolves; `settings: input.settings` is stored verbatim, GetJobTemplate/ListJobTemplates project the hollow graph back, and CreateJob with `JobTemplate` naming it inherits nothing.

The same hole exists on UpdateJobTemplate (`if let Some(settings) = input.settings { template.settings = settings }` with no validation), so a template that was created valid can be emptied by an update.

## Evidence

Smithy model: CreateJobTemplate declares `Settings` (JobTemplateSettings) as @required and declares BadRequestException (HTTP 400); `JobTemplateSettings$OutputGroups` is documented as "Contains one group of settings for each set of outputs that you want MediaConvert to create. ... one output group per output format" — a template with none describes no transcode. The already-filed CreateJob validation issue (20261024T010653Z-mediaconvert-createjob-validation) establishes that AWS validates the settings graph at submit time and that the service carries a dedicated `settingsValidationErrorsJsonBlob` envelope member for reporting it; a job template is validated by the same path. Handler: src/ops/create_job_template.rs — the only checks are the empty-Name 400, the empty-Queue 400 and the unknown-Queue 404; src/ops/update_job_template.rs replaces `settings` unchecked. Test coverage: tests/all/job_template_crud.rs and tests/all/job_templates.rs always post a structurally complete `settings` fixture (template_settings(inputs, groups)), so no test asserts what happens for an empty settings graph.

## Steps to Reproduce

1. Boot the emulator and point a mediaconvert client at it.
2. create_job_template(Name='empty', Settings={}) — expect BadRequestException (400) naming the settings problem and nothing stored; observe HTTP 201 with a jobTemplate whose settings is empty.
3. get_job_template(Name='empty') — observe the hollow template read back (and list_job_templates shows it).
4. create_job_template(Name='no-outputs', Settings={'OutputGroups': []}) — expect 400; observe 201.
5. Positive control: create_job_template with one FILE_GROUP_SETTINGS output group and one MP4/H_264 output must still succeed.
6. Also: create a valid template, then update_job_template(Name=<it>, Settings={}) — expect 400 and the stored settings untouched; observe 200 with the settings emptied.
