Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f

# UpdateJobTemplate replaces Settings with no validation: a template can be left with no output groups or with several inputs

_Filed: 2026-10-24T13:28:46Z_
_Operation: UpdateJobTemplate | Class: validation_

> _Note: this issue was identified against the code base at commit `d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateJobTemplate swaps the whole JobTemplateSettings graph in when the request carries one, without testing it at all. Two documented rules go unenforced:

1. `JobTemplateSettings$OutputGroups` describes the sets of outputs the template produces — a template whose settings carry no output group describes no transcode, and the real service refuses the request with BadRequestException (400) rather than storing it. The emulator answers HTTP 200 and persists it, after which every CreateJob that references the template inherits a graph with no outputs.
2. `JobTemplateSettings$Inputs` states "There can only be one input in a job template. Using the API, you can include multiple inputs when referencing a job template." The emulator accepts a template update carrying two or more inputs.

Expected: both are BadRequestException (400) with the stored template left exactly as it was. Actual: HTTP 200 with the invalid graph persisted and reported by GetJobTemplate/ListJobTemplates. (An already-open issue covers the same missing checks on CreateJobTemplate; this operation is a second, independent door to the same invalid state and is not covered by it.)

## Evidence

Smithy model: `JobTemplateSettings$Inputs` — "There can only be one input in a job template..."; `JobTemplateSettings$OutputGroups` — "Contains one group of settings for each set of outputs..."; UpdateJobTemplate (PUT /2017-08-29/jobTemplates/{Name} -> 200) declares BadRequestException httpError 400. Handler: src/ops/update_job_template.rs — the checks are the queue reference, the hop-destination queues and the hop wait range only; `if let Some(settings) = input.settings { template.settings = settings; }` applies the graph unconditionally, and `crate::state::job_settings_problem` (the one settings test this crate owns) is never called from this file. Test coverage: tests/all/job_template_crud.rs / job_templates.rs exercise only well-formed settings on the update path; no test asserts a refusal.

## Steps to Reproduce

1. Boot the emulator and point a mediaconvert client at it.
2. create_job_template(Name='t1', Settings={'Inputs':[{...one input...}],'OutputGroups':[{...one group with one output...}]}).
3. update_job_template(Name='t1', Settings={'Inputs':[{...one input...}]}) — i.e. a graph with no OutputGroups.
4. Expect BadRequestException (400) and get_job_template(Name='t1') still reporting the original output group; observe HTTP 200 and a stored template with no output groups.
5. Then update_job_template(Name='t1', Settings={'Inputs':[{...},{...},{...}],'OutputGroups':[{...}]}) — expect a 400 for more than one input in a job template, observe 200.
6. create_job(JobTemplate='t1', Role=<role>, Settings={}) to see the invalid graph propagate.
