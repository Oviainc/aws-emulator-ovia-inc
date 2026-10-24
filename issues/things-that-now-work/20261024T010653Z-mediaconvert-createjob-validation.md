Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# Reject a CreateJob whose Settings describe no inputs or outputs, and a JobTemplate reference that resolves to nothing

_Filed: 2026-10-24T01:06:52Z_
_Operation: CreateJob | Class: validation_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateJob accepts job settings that cannot describe a transcode and references that point at nothing, answering 201 and persisting a job AWS would never have created.

(a) `Settings` is accepted even when it carries neither `Inputs` nor `OutputGroups` (including `"settings": {}`). AWS validates the job settings graph at submit time and refuses such a request with BadRequestException (400) — the service even carries a dedicated error-envelope member for it (`settingsValidationErrorsJsonBlob`). Expected: a job with no inputs, or with no output groups and no JobTemplate to supply them, is a 400 and nothing is stored. Actual: `src/ops/create_job.rs` validates only that `Role` is non-empty; the settings structure is stored verbatim and projected back by GetJob/ListJobs.

(b) `JobTemplate` is stored without being resolved: a name no job template holds is accepted and echoed back on the Job. Expected: an unresolvable JobTemplate reference is NotFoundException (404), exactly as this handler already does for an unknown `Queue` reference and as CreateJobTemplate/UpdateJobTemplate already do for their `Queue` member. Actual: `job_template: input.job_template` with no store lookup, 201 with `jobTemplate` naming a template that does not exist.

## Evidence

Smithy model: CreateJob declares BadRequestException (400) and NotFoundException (404); `Settings` (JobSettings) is @required and its `Inputs` member is documented as "Use Inputs to define source file used in the transcode job", `OutputGroups` as "Contains one group of settings for each set of outputs" — a job with neither describes no work. The real service's error envelope carries `settingsValidationErrorsJsonBlob` (see the fixed issue 20261022T124827Z-mediaconvert-error-responses-omit-the-service-s-error...), which exists precisely to report settings validation failures. Handler: services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/create_job.rs — the only validation is `if input.role.trim().is_empty()` plus the `Queue` existence probe; `state.job_templates` is never consulted. Tests: no test in tests/all posts a job with empty settings or an unknown jobTemplate, so neither symptom is asserted either way (job_lifecycle.rs always sends inputs+outputGroups). Distinct from the open bundle 20261022T110844Z (cause 3), which is about MERGING a resolved template's settings into the job, not about refusing a reference that resolves to nothing or validating a settings graph.

## Steps to Reproduce

1. Boot the emulator and point a mediaconvert client at it.
2. create_job(Role=<role>, Settings={}) — expect BadRequestException (400) naming the settings problem; observe HTTP 201 with a SUBMITTED job whose settings is empty.
3. get_job(Id=<that id>) — observe the hollow settings read back.
4. create_job(Role=<role>, JobTemplate='no-such-template', Settings={'Inputs':[{'FileInput':'s3://b/in.mp4'}]}) — expect NotFoundException (404) as for an unknown Queue; observe HTTP 201 with jobTemplate 'no-such-template'.
5. Positive control: create_job with a template that exists must still succeed.
