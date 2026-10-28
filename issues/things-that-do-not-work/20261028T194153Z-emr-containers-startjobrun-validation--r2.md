Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f6f033d1876a0cb978cad56e1292384a0bfb4098

# StartJobRun accepts a jobDriver that names neither (or both) of the two job drivers

_Filed: 2026-10-28T19:41:53Z_
_Operation: StartJobRun | Class: validation_

> _Note: this issue was identified against the code base at commit `f6f033d1876a0cb978cad56e1292384a0bfb4098`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The `JobDriver` shape documents a cross-member rule the model cannot express as a trait: "Exactly one of the two available job drivers is required, either sparkSqlJobDriver or sparkSubmitJobDriver." Neither `sparkSubmitJobDriver` nor `sparkSqlJobDriver` carries `@required`, so the framework's constraint walk accepts `"jobDriver": {}` and also accepts a `jobDriver` carrying BOTH drivers.

The handler's only check on the driver is whether the member is ABSENT (`input.job_driver.is_none()`). A submission carrying `"jobDriver": {}` therefore passes, is persisted, answers 200, and a later `DescribeJobRun`/`ListJobRuns` reports a run whose `jobDriver` names no driver at all — precisely the outcome the comment above that check says it exists to prevent ("rather than becoming a run with no driver to run"). A submission carrying both drivers is likewise stored and reported.

The templated path has the same hole: `apply_job_template` maps each driver member only when it is present (`spark_submit_job_driver: driver.spark_submit_job_driver.as_ref().map(...)`), and the resolved values are then put through `validate_to_message`, which enforces only the modelled `@length`/`@pattern`/enum traits — none of which expresses the "exactly one" rule. So a stored template whose `jobDriver` names neither driver also produces a driverless run.

## Evidence

Smithy model, shape `JobDriver` (smithy_query query="doc" shape="JobDriver"):
  "Specify the driver that the job runs on. Exactly one of the two available job drivers is required, either sparkSqlJobDriver or sparkSubmitJobDriver."
Same query, query="shape": members `sparkSubmitJobDriver` and `sparkSqlJobDriver` are BOTH optional (no `required` flag), so nothing before the handler enforces the sentence.

The handler checks only presence of the member — services/service-com-amazonaws-emrcontainers-awschicagowebservice/src/ops/start_job_run.rs:307-313:
```
        // itself; one missing any of them is refused and nothing is stored, rather than
        // becoming a run with no driver to run.
        if templated.is_none() {
            let missing: Vec<&str> = [
                ("executionRoleArn", input.execution_role_arn.is_none()),
                ("releaseLabel", input.release_label.is_none()),
                ("jobDriver", input.job_driver.is_none()),
```
Nothing anywhere in the crate inspects the two driver members for a refusal: `grep -n 'spark_sql_job_driver|spark_submit_job_driver' src/` matches only the substitution walk (src/ops/start_job_run.rs:667, 682-683) and the type declaration (src/ops/types/shared_types.rs:76-77).

Test coverage: tests/all/start_job_run_required_members.rs:25-40 pins only the member-ABSENT case ("Every non-empty subset of [`REQUIRED`]", with `drivers()` supplying one populated driver per row), so no existing test pins an empty or doubly-filled `jobDriver` as acceptable.

## Steps to Reproduce

1. Create a virtual cluster: POST /virtualclusters with {"name":"vc","clientToken":"t1","containerProvider":{"id":"eks-1","type":"EKS","info":{"eksInfo":{"namespace":"emr"}}}} and note the returned id.
2. POST /virtualclusters/{id}/jobruns with {"clientToken":"t2","executionRoleArn":"arn:aws:iam::000000000000:role/EMRJobRole","releaseLabel":"emr-6.15.0-latest","jobDriver":{}} — expect ValidationException (HTTP 400) because the model requires exactly one of the two drivers; observe HTTP 200 with a minted job-run id instead.
3. GET /virtualclusters/{id}/jobruns/{runId} — observe the stored run reports a `jobDriver` that names neither `sparkSubmitJobDriver` nor `sparkSqlJobDriver`.
4. Repeat step 2 with {"jobDriver":{"sparkSubmitJobDriver":{"entryPoint":"s3://b/j.py"},"sparkSqlJobDriver":{"entryPoint":"s3://b/q.sql"}}} — expect ValidationException for "exactly one"; observe HTTP 200 and both drivers stored.
