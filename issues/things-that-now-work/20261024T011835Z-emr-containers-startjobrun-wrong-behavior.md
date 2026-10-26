Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# StartJobRun refuses every jobTemplateId with ValidationException instead of starting the run the template describes

_Filed: 2026-10-24T01:18:35Z_
_Operation: StartJobRun | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected (AWS): `StartJobRun` with a `jobTemplateId` that names a stored job template starts a job run built from that template's `jobTemplateData` — `executionRoleArn`, `releaseLabel`, `jobDriver` and `configurationOverrides` come from the template, any `${param}` placeholder it declares in `parameterConfiguration` is substituted from `jobTemplateParameters` (falling back to the parameter's `defaultValue`), the template's `jobTags` are applied, and the call answers 200 with the new run's `id`/`arn`. A later `DescribeJobRun`/`ListJobRuns` reports a run carrying those template-derived values. `jobTemplateParameters` is likewise usable.

Actual (emulator): the handler resolves the template out of state only to refuse the submission. If the id resolves it returns HTTP 400 `ValidationException` ("job template <id> cannot be applied to a job run by this emulator yet"); if `jobTemplateParameters` is present without `jobTemplateId` it returns HTTP 400 `ValidationException`. So the entire job-template submission path — the reason `CreateJobTemplate`/`DescribeJobTemplate` exist — can never produce a job run: a client can create a template but never run it.

## Evidence

Spec: the Smithy model binds `StartJobRun` input members `jobTemplateId` (ResourceIdString) and `jobTemplateParameters` (map, len 0..100), and `JobTemplateData` (CreateJobTemplate input) declares `executionRoleArn`, `releaseLabel` and `jobDriver` as @required plus a `parameterConfiguration` map of `TemplateParameterConfiguration` (type STRING/NUMBER + defaultValue) — i.e. a template carries exactly the values a run needs, and the documented purpose of `JobTemplateData` is "The values of StartJobRun API requests used in job runs started using the job template". `StartJobRun` models `ValidationException` only for invalid requests, not for a well-formed template reference.

Handler: services/service-com-amazonaws-emrcontainers-awschicagowebservice/src/ops/start_job_run.rs lines 199-230 — after `state.job_templates.contains_key(template_id)` succeeds the handler unconditionally returns `ValidationException` (comment: "APPLYING it ... is not implemented in this slice"), and lines 223-230 refuse `jobTemplateParameters` outright.

Tests: the current (divergent) refusal is locked in by tests/all/job_template_idempotency.rs:709-752 (`start_job_run_resolves_a_job_template_id_out_of_the_store`, asserts 400 ValidationException for a stored template) and tests/all/virtual_cluster_job_run_loop.rs:781-800. No test asserts the AWS behavior of a template-backed run.

## Steps to Reproduce

1. POST /virtualclusters with {name, clientToken, containerProvider:{id,type:EKS}} and keep the returned cluster id.
2. POST /jobtemplates with {name, clientToken, jobTemplateData:{executionRoleArn, releaseLabel, jobDriver:{sparkSubmitJobDriver:{entryPoint:"s3://b/j.py"}}}} and keep the returned template id.
3. POST /virtualclusters/{clusterId}/jobruns with {clientToken:"t1", jobTemplateId:"<templateId>"}.
4. Observed: HTTP 400 with x-amzn-errortype: ValidationException and no job run created. Expected: HTTP 200 with a new job run id, and GET /virtualclusters/{clusterId}/jobruns/{id} reporting the jobDriver/releaseLabel/executionRoleArn taken from the template.
5. Repeat with a template that declares parameterConfiguration {"rows":{type:NUMBER,defaultValue:"10"}} and pass jobTemplateParameters {"rows":"20"}; expected the substituted value in the run, observed 400.
