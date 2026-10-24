Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# Built-in SYSTEM job templates are reported with an empty Settings graph

_Filed: 2026-10-24T01:06:52Z_
_Operation: ListJobTemplates | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListJobTemplates?listBy=SYSTEM` is the documented way to discover the account's built-in templates, and the emulator does seed four of them — but each is seeded with `JobTemplateSettings::default()`, i.e. no input templates and no output groups. Expected: a built-in template reports the encoding configuration AWS publishes for it (at minimum one output group with its outputs/container settings), so a client can list the SYSTEM catalogue, pick an entry, and create a job from it. Actual: ListJobTemplates(listBy=SYSTEM) and GetJobTemplate on one of those names return a template whose `settings` is an empty object, so the catalogue is name-and-timestamps only; a job created from a system template inherits nothing at all (compounding the already-filed template-merge gap). The same four names are the entire catalogue, where AWS publishes a substantially larger set.

## Evidence

Smithy model: `JobTemplateListBy` includes SYSTEM and `JobTemplate$Type` documents system templates as built-ins the user "can't modify or delete"; `JobTemplate$Settings` (JobTemplateSettings) is the template's encoding configuration. State: src/state.rs `SYSTEM_JOB_TEMPLATES` is a 4-name array and `JobTemplateRecord::system()` sets `settings: JobTemplateSettings::default()`, with the module comment conceding "They carry no encoder settings". Handler: src/ops/list_job_templates.rs selects TYPE_SYSTEM records for listBy=SYSTEM and projects them through `record.view(&region)`. Tests: tests/all/job_templates.rs and resource_projection_defaults.rs assert only the ordering/type/default scalar members of the SYSTEM entries (priority, statusUpdateInterval, acceleration), never that their settings are empty, so nothing asserts this behavior.

## Steps to Reproduce

1. Boot the emulator and point a mediaconvert client at it.
2. list_job_templates(ListBy='SYSTEM') — note the entries returned.
3. Inspect JobTemplates[0].Settings: expect a populated graph (OutputGroups with outputs/container settings) as AWS returns for a built-in template; observe an empty settings object.
4. get_job_template(Name=<that system name>) — same empty settings.
5. create_job(Role=<role>, JobTemplate=<that system name>, Settings={'Inputs':[{'FileInput':'s3://b/in.mp4'}]}) then get_job — the job inherits no outputs from the built-in template.
