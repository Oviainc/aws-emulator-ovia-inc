Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f6f033d1876a0cb978cad56e1292384a0bfb4098

# CreateJobTemplate stores a jobTemplateData.jobDriver that names neither of the two job drivers

_Filed: 2026-10-28T19:41:53Z_
_Operation: CreateJobTemplate | Class: validation_

> _Note: this issue was identified against the code base at commit `f6f033d1876a0cb978cad56e1292384a0bfb4098`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`JobTemplateData$jobDriver` targets `JobDriver`, whose documentation states "Exactly one of the two available job drivers is required, either sparkSqlJobDriver or sparkSubmitJobDriver." Both members of `JobDriver` are optional in the model, so the framework accepts `"jobDriver": {}` inside `jobTemplateData`, and the handler persists `input.job_template_data` without looking at it. `DescribeJobTemplate`/`ListJobTemplates` then report a template whose driver names nothing, and a `StartJobRun` naming that template mints a run with no driver.

This is the one shape-level "a template must be able to produce a run" rule the crate does not apply, while its sibling rule in the same code path IS applied: `run_configuration_overrides` refuses a stored template that configures a monitoring destination without naming it, with `missing_template_member` — "Job template {template_id} must specify configurationOverrides.{member} for a job run to be started from it."

## Evidence

Smithy model, shape `JobDriver` (smithy_query query="doc" shape="JobDriver"):
  "Specify the driver that the job runs on. Exactly one of the two available job drivers is required, either sparkSqlJobDriver or sparkSubmitJobDriver."
Smithy model, shape `JobTemplateData`: `jobDriver` is `{"target": "JobDriver", "required": true}` — requiredness of the member, not of its content; both `JobDriver` members stay optional.

Stored unchecked — services/service-com-amazonaws-emrcontainers-awschicagowebservice/src/ops/create_job_template.rs:242:
```
            job_template_data: input.job_template_data,
```
(the handler's only refusals before it are the idempotency replay at :188-206 and the store cap at :214-224).

The sibling rule the crate DOES enforce for a template that cannot produce a run — src/ops/start_job_run.rs:788-794:
```
/// The refusal for a template that configures something a job run needs a value for and
/// does not name it.
fn missing_template_member(template_id: &str, member: &str) -> String {
    format!(
        "Job template {template_id} must specify configurationOverrides.{member} for a job run to be started from it."
    )
}
```

Test coverage: tests/all/job_template_lifecycle.rs and tests/all/job_template_idempotency.rs always build `jobTemplateData` with a populated `sparkSubmitJobDriver` (e.g. the fixture quoted at tests/all/shared_listing_filters.rs:97-103: `"jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/filters.py"}}`), so no test pins an empty `jobDriver` as acceptable.

## Steps to Reproduce

1. POST /jobtemplates with {"name":"tpl","clientToken":"t1","jobTemplateData":{"executionRoleArn":"arn:aws:iam::000000000000:role/EMRJobRole","releaseLabel":"emr-6.15.0-latest","jobDriver":{}}} — expect ValidationException (HTTP 400) because the model requires exactly one of the two drivers; observe HTTP 200 with a minted template id.
2. GET /jobtemplates/{templateId} — observe `jobTemplateData.jobDriver` comes back naming neither driver.
3. Create a virtual cluster, then POST /virtualclusters/{vcId}/jobruns with {"clientToken":"t2","jobTemplateId":"{templateId}"} — observe HTTP 200 and a stored job run with no driver.
