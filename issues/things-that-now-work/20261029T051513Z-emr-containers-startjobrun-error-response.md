Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: a0bcc3233750d466bb98593c36c54004977c8656

# Job-template refusals quote caller-supplied parameter names and values whole, so ValidationException.message runs past its modelled 1024-character bound

_Filed: 2026-10-29T05:15:13Z_
_Operation: StartJobRun | Class: error-response_

> _Note: this issue was identified against the code base at commit `a0bcc3233750d466bb98593c36c54004977c8656`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every refusal `apply_job_template` produces is handed straight to the modelled `ValidationException` as its `message`, built by interpolating caller-supplied `jobTemplateParameters` keys and values verbatim. The model bounds that member at 1024 characters, but the values interpolated into it are bounded at 512 (a parameter name) and 1024 (a parameter value) EACH, and up to 100 of them may be named in one request, so three ordinary refusal paths can emit a `message` well past the modelled maximum:

1. a value that is not a number for a `NUMBER`-typed parameter quotes BOTH the name (<=512) and the value (<=1024) — a single 1024-character value already overflows the bound on its own;
2. supplied parameters the template does not declare are joined and quoted in full, so three 512-character keys render ~1540 characters of names alone;
3. the unresolved-reference refusal joins every parameter name the walk collected.

The handler applies no truncation anywhere (there is no `truncate` or character-budget call in the crate outside the substitution budget), so the client receives an error whose `message` cannot be held by the shape the model declares for it. Nothing in the crate justifies the divergence: the other refusals of this crate interpolate only values the model already bounds below 1024 (a `ResourceIdString` of <=64, an `RsiArn` of <=500).

## Evidence

Model (smithy_query query="shape" shape="ValidationException"): `"message": {"target": "String1024", "length": {"min": 1, "max": 1024}, "pattern": "\\S"}` — and the operation declares that very error: OPERATIONS.md:183 `- **Errors:** ... ValidationException (HTTP 400)`.

Model (smithy_query query="shape" shape="TemplateParameterInputMap"): `"length": {"min": 0, "max": 100}, "key": {"target": "TemplateParameterName", "length": {"min": 1, "max": 512}, ...}, "value": {"target": "String1024", "length": {"min": 1, "max": 1024}, ...}` — so a single request may legally carry 100 names of 512 characters and 100 values of 1024 characters.

Handler, the three messages built from those values:
- services/service-com-amazonaws-emrcontainers-awschicagowebservice/src/ops/start_job_run.rs:871-873 — `return Err(format!("Job template parameter {name} is declared as {PARAMETER_TYPE_NUMBER}, so {value} is not a valid value for it."));`
- services/service-com-amazonaws-emrcontainers-awschicagowebservice/src/ops/start_job_run.rs:846-849 — `return Err(format!("Job template {template_id} declares no parameter named [{}].", undeclared.into_iter().collect::<Vec<&str>>().join(", ")));`
- services/service-com-amazonaws-emrcontainers-awschicagowebservice/src/ops/start_job_run.rs:916-924 — `return Err(format!("Job template {template_id} references parameter(s) [{}] that have no value: ...", ...));`

And each is lifted verbatim into the modelled error, with no length check, at services/service-com-amazonaws-emrcontainers-awschicagowebservice/src/ops/start_job_run.rs:278-282:
```
.map_err(|message| {
    StartJobRunErrorModeled::ValidationException(ValidationException {
        message: Some(message),
    })
})?
```

No crate comment, test or doc justifies exceeding the bound: the crate's own `MAX_SUBSTITUTED_CHARACTERS` doc (start_job_run.rs:524-534) is about what the SUBSTITUTION may render, not about the refusal text, and the suite that pins template refusals (tests/all/start_job_run_job_template.rs) uses only short parameter names and values, so bounding the message leaves every existing assertion intact.

## Steps to Reproduce

1. CreateJobTemplate with `jobTemplateData.parameterConfiguration` declaring one parameter `p` of `type: "NUMBER"`, and a `jobDriver` that references `${p}`.
2. StartJobRun against a RUNNING virtual cluster with `jobTemplateId` naming that template and `jobTemplateParameters = {"p": "<1024 'x' characters>"}` (legal: `String1024` allows 1024 characters, pattern `\S`).
3. Observe the 400 ValidationException: its `message` is ~1090 characters — longer than the 1024-character maximum `ResourceNotFoundException`/`ValidationException` `message` (`String1024`) declares.
4. Alternative path: StartJobRun with `jobTemplateParameters` carrying three keys of 512 characters each that the template declares nothing about; the "declares no parameter named [...]" message renders ~1600 characters.
